//! ARENA RUSH — Solana Blitz v10 entry.
//!
//! 30-second real-time reflex game. The arena PDA is delegated to a MagicBlock
//! Ephemeral Rollup for gasless 10 ms-block tap play; round end requests
//! verifiable randomness from the delegated VRF queue; the callback resolves a
//! loot multiplier; commit-and-undelegate settles state back on base where the
//! SPL reward (score x multiplier, capped) is minted to the player.
//!
//! Full build (v1): 11 instructions. Design-review fixes applied (see
//! ../BUILD.md §1): VRF timeout recovery (`force_expire`), RESOLVED guard on
//! commit, 30 s round-duration enforcement via timestamps, u16::MAX target
//! sentinel, per-round reward cap, mint-on-claim SPL.

use anchor_lang::prelude::*;
use anchor_spl::{
    associated_token::AssociatedToken,
    token::{self, Mint, Token, TokenAccount},
};
use ephemeral_rollups_sdk::anchor::{commit, delegate, ephemeral, vrf, vrf_callback};
use ephemeral_rollups_sdk::cpi::DelegateConfig;
use ephemeral_rollups_sdk::ephem::MagicIntentBundleBuilder;
use ephemeral_rollups_sdk::vrf::{
    self,
    consts::{program_identity_pubkey, scoped_vrf_identity_const},
    instructions::{
        create_request_high_priority_scoped_randomness_ix_const, RequestRandomnessParams,
    },
    types::SerializableAccountMeta,
};

declare_id!("HDAvFeo4tpoGzmZqvAhRzmYVdF1qi4CCJxQNZjhQj9D2");

/// Requester identity PDA, derived at compile time (SDK 0.18+).
const PROGRAM_IDENTITY: Pubkey = program_identity_pubkey(&crate::ID);

/// Scoped VRF identity PDA (`PDA([IDENTITY, program_id], vrf)`), derived at
/// compile time. MUST stay a `const` item: the `#[vrf_callback]` macro's
/// inlined `scoped_vrf_identity_const(&ID)` address expression is evaluated
/// at RUNTIME in anchor's generated validation code, and the PDA bump search
/// (pure-Rust ed25519 arithmetic, no syscalls) burns the whole 200k CU
/// callback budget — the callback OOMs before the handler runs (proven by
/// MagicSVM test-06: 197,000/197,000 CU consumed). A const item folds to a
/// plain 32-byte comparison.
const SCOPED_VRF_IDENTITY: Pubkey =
    Pubkey::new_from_array(scoped_vrf_identity_const(&crate::ID).0);

pub const ARENA_SEED: &[u8] = b"arena";
pub const REWARD_MINT_SEED: &[u8] = b"reward_mint";
pub const REWARD_MINT_AUTH_SEED: &[u8] = b"reward_mint_auth";
/// Anti-replay ledger seed (audit fix M-4): the player_stats PDA is NEVER
/// delegated, so it survives any undelegation snapshot revert.
pub const PLAYER_STATS_SEED: &[u8] = b"player_stats";

/// Round states.
pub const STATE_IDLE: u8 = 0; // created, not playing
pub const STATE_PLAYING: u8 = 1; // delegated, round live
pub const STATE_ROLLING: u8 = 2; // round ended, VRF request pending
pub const STATE_RESOLVED: u8 = 3; // VRF callback landed (or expired), multiplier set
pub const STATE_CLAIMED: u8 = 4; // reward claimed on base

/// Max scored taps per ER slot (10 ms). A human taps ~5-10/s; 1/slot caps
/// bots at 100/s while never throttling a real player.
pub const MAX_TAPS_PER_SLOT: u8 = 1;

/// Round length (seconds) + grace for in-flight taps / clock skew.
pub const ROUND_DURATION_SECS: i64 = 30;
pub const ROUND_GRACE_SECS: i64 = 5;

/// VRF request TTL is ~120 s; after this the round may be force-expired
/// (fallback multiplier x1) instead of staying stuck in ROLLING.
pub const VRF_TIMEOUT_SECS: i64 = 130;

/// Max RUSH tokens minted per round (score x multiplier, capped).
pub const MAX_REWARD: u64 = 1_000;

/// Sentinel for "no target tapped yet" (target ids start at 0).
pub const NO_TARGET: u16 = u16::MAX;

#[ephemeral]
#[program]
pub mod arena_rush {
    use super::*;

    /// Create the player's arena PDA on the base layer.
    pub fn initialize(ctx: Context<Initialize>) -> Result<()> {
        let arena = &mut ctx.accounts.arena;
        arena.player = ctx.accounts.player.key();
        arena.score = 0;
        arena.state = STATE_IDLE;
        arena.multiplier = 0;
        arena.pending_nonce = 0;
        arena.round_nonce = 0;
        arena.last_tap_slot = 0;
        arena.taps_this_slot = 0;
        arena.last_target_id = NO_TARGET;
        arena.best_score = 0;
        arena.round_start_ts = 0;
        arena.vrf_request_ts = 0;
        arena.bump = ctx.bumps.arena;
        Ok(())
    }

    /// Create the RUSH reward mint (decimals 0). One-time: the mint is a PDA
    /// so `init` fails if it already exists. Permissionless to call — the
    /// mint authority is the program PDA either way, so there is no griefing
    /// value in front-running the setup call.
    pub fn init_reward_mint(_ctx: Context<InitRewardMint>) -> Result<()> {
        msg!("RUSH reward mint initialized");
        Ok(())
    }

    /// Delegate the arena PDA to the ER (base-layer tx).
    ///
    /// Audit fix H-1: the arena must be IDLE or CLAIMED. A RESOLVED round
    /// re-delegated here would be bricked forever — `claim_reward` needs
    /// the arena on base, and `start_round` needs IDLE|CLAIMED, so the
    /// reward could never be claimed. The guard deserializes the raw
    /// AccountInfo because the `#[delegate]` macro requires it.
    pub fn delegate(ctx: Context<DelegateArena>) -> Result<()> {
        // The data borrow MUST drop before the delegate CPI: the CPI
        // borrows the same account mutably, and a live Ref fails with
        // AccountBorrowFailed (caught by MagicSVM test run 1).
        let state = {
            let arena_data = ctx.accounts.arena.data.borrow();
            let mut arena_bytes: &[u8] = &arena_data;
            Arena::try_deserialize(&mut arena_bytes)?.state
        };
        require!(
            state == STATE_IDLE || state == STATE_CLAIMED,
            ArenaError::CannotDelegateUnclaimed
        );
        ctx.accounts.delegate_arena(
            &ctx.accounts.player,
            &[ARENA_SEED, ctx.accounts.player.key().as_ref()],
            DelegateConfig::default(),
        )?;
        Ok(())
    }

    /// Start (or restart) a round on the ER: resets score/state for a fresh
    /// 30-second round. Only from IDLE or CLAIMED (a resolved round must be
    /// claimed first; a ROLLING round must resolve or expire first).
    pub fn start_round(ctx: Context<StartRound>) -> Result<()> {
        let arena = &mut ctx.accounts.arena;
        require!(
            arena.state == STATE_IDLE || arena.state == STATE_CLAIMED,
            ArenaError::RoundNotStartable
        );
        let now = Clock::get()?.unix_timestamp;
        arena.score = 0;
        arena.state = STATE_PLAYING;
        arena.multiplier = 0;
        arena.pending_nonce = 0;
        arena.last_tap_slot = 0;
        arena.taps_this_slot = 0;
        arena.last_target_id = NO_TARGET;
        arena.round_start_ts = now;
        arena.vrf_request_ts = 0;
        // Audit fix L-5: checked_add — a saturating nonce would let two
        // rounds share a nonce (and a caller_seed) at u64::MAX.
        arena.round_nonce = arena
            .round_nonce
            .checked_add(1)
            .ok_or(ArenaError::NonceOverflow)?;
        Ok(())
    }

    /// Score one tap on the ER. Enforces the per-slot tap cap, rejects
    /// replaying the same target id twice in a row, and enforces the 30 s
    /// round window (timestamp-based: ER-vs-base slot semantics are
    /// unverified, wall-clock is not). Plausible fairness, not production
    /// anti-cheat.
    pub fn tap(ctx: Context<Tap>, target_id: u16) -> Result<()> {
        let arena = &mut ctx.accounts.arena;
        require!(arena.state == STATE_PLAYING, ArenaError::NotPlaying);
        let now = Clock::get()?.unix_timestamp;
        require!(
            now.saturating_sub(arena.round_start_ts) <= ROUND_DURATION_SECS + ROUND_GRACE_SECS,
            ArenaError::RoundTimeExpired
        );
        require!(target_id != arena.last_target_id, ArenaError::TargetReplay);
        let slot = Clock::get()?.slot;
        if slot == arena.last_tap_slot {
            require!(
                arena.taps_this_slot < MAX_TAPS_PER_SLOT,
                ArenaError::TapRateLimited
            );
            arena.taps_this_slot = arena.taps_this_slot.saturating_add(1);
        } else {
            arena.last_tap_slot = slot;
            arena.taps_this_slot = 1;
        }
        arena.last_target_id = target_id;
        arena.score = arena.score.saturating_add(1);
        Ok(())
    }

    /// Close the round and request randomness from the delegated VRF queue
    /// (fee-exempt inside the ER). Exactly one live request per round.
    pub fn end_round(ctx: Context<EndRound>) -> Result<()> {
        let arena_key = ctx.accounts.arena.key();
        let arena = &mut ctx.accounts.arena;
        require!(arena.state == STATE_PLAYING, ArenaError::NotPlaying);
        arena.state = STATE_ROLLING;
        arena.pending_nonce = arena.round_nonce;
        arena.vrf_request_ts = Clock::get()?.unix_timestamp;
        let round_nonce = arena.round_nonce;

        // Caller seed (audit fix M-3): fill all 32 bytes deterministically
        // and uniquely per player + arena + round — player bytes [0..8],
        // arena bytes [0..8], round nonce, player bytes [8..16]. The old
        // seed used only the round nonce, so every player shared identical
        // seeds per round number. NOTE (audit M-2): this does NOT
        // cryptographically bind the request to its origin — any VRF
        // request bound to this arena+nonce can still resolve the round;
        // the outcome stays oracle-uniform either way.
        let mut caller_seed = [0u8; 32];
        let player_key = ctx.accounts.player.key();
        caller_seed[0..8].copy_from_slice(&player_key.as_ref()[0..8]);
        caller_seed[8..16].copy_from_slice(&arena_key.as_ref()[0..8]);
        caller_seed[16..24].copy_from_slice(&round_nonce.to_le_bytes());
        caller_seed[24..32].copy_from_slice(&player_key.as_ref()[8..16]);

        let params = RequestRandomnessParams {
            payer: ctx.accounts.player.key(),
            oracle_queue: ctx.accounts.oracle_queue.key(),
            callback_program_id: ID,
            callback_discriminator: instruction::VrfCallback::DISCRIMINATOR.to_vec(),
            caller_seed,
            accounts_metas: Some(vec![SerializableAccountMeta {
                pubkey: arena_key,
                is_signer: false,
                is_writable: true,
            }]),
            callback_args: Some(round_nonce.to_le_bytes().to_vec()),
            ..Default::default()
        };
        // Audit fix L-7: high-priority VRF request (discriminator 11) so
        // the callback lands in ~3 ER slots instead of queueing behind
        // regular-priority requests.
        let ix = create_request_high_priority_scoped_randomness_ix_const(params, PROGRAM_IDENTITY);
        ctx.accounts
            .invoke_signed_vrf(&ctx.accounts.player.to_account_info(), &ix)?;
        Ok(())
    }

    /// VRF callback: resolve the loot multiplier from the randomness.
    /// Bound to the request nonce; rejects superseded/late callbacks and is
    /// idempotent (a duplicate callback for an already-resolved round fails
    /// the state check, never a double payout).
    pub fn vrf_callback(
        ctx: Context<VrfCallback>,
        randomness: [u8; 32],
        nonce: u64,
    ) -> Result<()> {
        let arena = &mut ctx.accounts.arena;
        require!(arena.state == STATE_ROLLING, ArenaError::NotRolling);
        require!(nonce == arena.pending_nonce, ArenaError::StaleCallback);
        // Loot tiers: Common x1 55% / Rare x2 30% / Epic x3 11% / Legendary x5 4%.
        // random_u8_with_range is INCLUSIVE on both ends ([min, max]), so
        // (0, 99) gives exactly 100 values: 0..=54 (55), 55..=84 (30),
        // 85..=95 (11), 96..=99 (4). (0, 100) would give 101 values and a
        // 5% Legendary rate — caught in pre-submit review.)
        let roll = vrf::rnd::random_u8_with_range(&randomness, 0, 99);
        arena.multiplier = match roll {
            0..=54 => 1,
            55..=84 => 2,
            85..=95 => 3,
            _ => 5,
        };
        if arena.score > arena.best_score {
            arena.best_score = arena.score;
        }
        arena.state = STATE_RESOLVED;
        Ok(())
    }

    /// Timeout recovery: if the VRF callback never arrives (request TTL
    /// ~120 s, no refund), the round can be resolved after 130 s with the
    /// fallback Common multiplier (x1) instead of staying stuck in ROLLING
    /// forever. A late real callback afterwards is rejected (state is
    /// RESOLVED, not ROLLING). Audit fix M-1: PERMISSIONLESS — anyone may
    /// call it (the caller pays gas); the arena PDA seeds bind it to the
    /// player, so no one else's round can be touched. An abandoned ROLLING
    /// arena no longer locks forever.
    pub fn force_expire(ctx: Context<ForceExpire>) -> Result<()> {
        let arena = &mut ctx.accounts.arena;
        require!(arena.state == STATE_ROLLING, ArenaError::NotRolling);
        let now = Clock::get()?.unix_timestamp;
        require!(
            now.saturating_sub(arena.vrf_request_ts) > VRF_TIMEOUT_SECS,
            ArenaError::VrfNotExpired
        );
        arena.multiplier = 1;
        if arena.score > arena.best_score {
            arena.best_score = arena.score;
        }
        arena.state = STATE_RESOLVED;
        Ok(())
    }

    /// Commit state and undelegate the arena PDA (ER tx). Only from RESOLVED:
    /// committing a ROLLING round would freeze an unresolved round on base.
    pub fn commit_and_undelegate(ctx: Context<CommitAndUndelegate>) -> Result<()> {
        let arena = &ctx.accounts.arena;
        require!(arena.state == STATE_RESOLVED, ArenaError::NotResolved);
        MagicIntentBundleBuilder::new(
            ctx.accounts.player.to_account_info(),
            ctx.accounts.magic_context.to_account_info(),
            ctx.accounts.magic_program.to_account_info(),
        )
        .commit_and_undelegate(&[ctx.accounts.arena.to_account_info()])
        .build_and_invoke()?;
        Ok(())
    }

    /// Mint the SPL reward on base: min(score x multiplier, MAX_REWARD) RUSH
    /// (decimals 0) to the player's ATA. State flips to CLAIMED before the
    /// mint CPI (and the whole ix is atomic), so a second claim tx always
    /// sees CLAIMED.
    ///
    /// Audit fix M-4 (hardening): the claim is also recorded in the
    /// never-delegated `player_stats` PDA (`last_claimed_nonce`), so even
    /// if an undelegation ever reverted the base arena snapshot to
    /// RESOLVED, the round could not be claimed twice.
    pub fn claim_reward(ctx: Context<ClaimReward>) -> Result<()> {
        let arena = &mut ctx.accounts.arena;
        require!(arena.state == STATE_RESOLVED, ArenaError::NotResolved);
        require!(
            arena.round_nonce > ctx.accounts.player_stats.last_claimed_nonce,
            ArenaError::RoundAlreadyClaimed
        );
        let amount = (arena.score as u64)
            .saturating_mul(arena.multiplier as u64)
            .min(MAX_REWARD);
        let round_nonce = arena.round_nonce;
        arena.state = STATE_CLAIMED;
        let stats = &mut ctx.accounts.player_stats;
        stats.last_claimed_nonce = round_nonce;
        stats.total_minted = stats.total_minted.saturating_add(amount);
        stats.bump = ctx.bumps.player_stats;

        let auth_seeds: &[&[u8]] = &[
            REWARD_MINT_AUTH_SEED,
            &[ctx.bumps.mint_authority],
        ];
        // anchor-lang 1.x: CpiContext::new_with_signer takes the program
        // Pubkey (not AccountInfo) as its first argument.
        token::mint_to(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.key(),
                token::MintTo {
                    mint: ctx.accounts.reward_mint.to_account_info(),
                    to: ctx.accounts.player_ata.to_account_info(),
                    authority: ctx.accounts.mint_authority.to_account_info(),
                },
                &[auth_seeds],
            ),
            amount,
        )?;
        msg!("Claimed {} RUSH (score {} x{})", amount, arena.score, arena.multiplier);
        Ok(())
    }
}

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(
        init,
        payer = player,
        space = 8 + Arena::INIT_SPACE,
        seeds = [ARENA_SEED, player.key().as_ref()],
        bump
    )]
    pub arena: Account<'info, Arena>,
    #[account(mut)]
    pub player: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct InitRewardMint<'info> {
    #[account(
        init,
        payer = payer,
        seeds = [REWARD_MINT_SEED],
        bump,
        mint::decimals = 0,
        mint::authority = mint_authority,
        // Audit fix L-4: NO freeze authority (omitted = None). No
        // instruction ever freezes RUSH; a live freeze authority was a
        // dormant privilege. (The live devnet mint keeps its existing
        // freeze authority — mint fields are immutable after creation.)
    )]
    pub reward_mint: Account<'info, Mint>,
    /// CHECK: PDA mint authority; only used as a CPI signer via seeds.
    #[account(seeds = [REWARD_MINT_AUTH_SEED], bump)]
    pub mint_authority: UncheckedAccount<'info>,
    #[account(mut)]
    pub payer: Signer<'info>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[delegate]
#[derive(Accounts)]
pub struct DelegateArena<'info> {
    #[account(mut)]
    pub player: Signer<'info>,
    /// CHECK: The PDA to delegate (AccountInfo + `del`, per SDK 0.17.1+).
    #[account(mut, del, seeds = [ARENA_SEED, player.key().as_ref()], bump)]
    pub arena: AccountInfo<'info>,
}

#[derive(Accounts)]
pub struct StartRound<'info> {
    #[account(mut, seeds = [ARENA_SEED, player.key().as_ref()], bump = arena.bump, has_one = player)]
    pub arena: Account<'info, Arena>,
    pub player: Signer<'info>,
}

#[derive(Accounts)]
pub struct Tap<'info> {
    #[account(mut, seeds = [ARENA_SEED, player.key().as_ref()], bump = arena.bump, has_one = player)]
    pub arena: Account<'info, Arena>,
    pub player: Signer<'info>,
}

#[vrf]
#[derive(Accounts)]
pub struct EndRound<'info> {
    #[account(mut)]
    pub player: Signer<'info>,
    #[account(mut, seeds = [ARENA_SEED, player.key().as_ref()], bump = arena.bump, has_one = player)]
    pub arena: Account<'info, Arena>,
    /// CHECK: The oracle queue — pinned to the delegated ephemeral queue
    /// (the only queue the ER request path uses). Test/base queues are
    /// rejected: their randomness guarantees on devnet are unverified, and
    /// a player must not be able to shop for a weaker queue.
    #[account(
        mut,
        constraint = oracle_queue.key() == vrf::consts::DEFAULT_EPHEMERAL_QUEUE
    )]
    pub oracle_queue: UncheckedAccount<'info>,
}

#[vrf_callback]
#[derive(Accounts)]
pub struct VrfCallback<'info> {
    /// Scoped VRF identity PDA, declared explicitly so the macro does NOT
    /// inject its own field (its inlined address expression evaluates the
    /// PDA derivation at runtime and OOMs the callback — see
    /// SCOPED_VRF_IDENTITY). Its presence as a signer proves the callback
    /// was issued by the VRF program for this program.
    #[account(address = SCOPED_VRF_IDENTITY)]
    pub vrf_program_identity: Signer<'info>,
    #[account(mut)]
    pub arena: Account<'info, Arena>,
}

#[derive(Accounts)]
pub struct ForceExpire<'info> {
    // Audit fix M-1: permissionless — the arena PDA seeds bind it to the
    // player pubkey, so no has_one / player-signer is needed. `player` is
    // read-only and used only for seeds derivation; `caller` is any signer
    // who pays gas.
    #[account(mut, seeds = [ARENA_SEED, player.key().as_ref()], bump = arena.bump)]
    pub arena: Account<'info, Arena>,
    /// CHECK: Player pubkey, read-only, used only to derive the arena PDA.
    pub player: UncheckedAccount<'info>,
    pub caller: Signer<'info>,
}

#[commit]
#[derive(Accounts)]
pub struct CommitAndUndelegate<'info> {
    #[account(mut)]
    pub player: Signer<'info>,
    #[account(mut, seeds = [ARENA_SEED, player.key().as_ref()], bump = arena.bump, has_one = player)]
    pub arena: Account<'info, Arena>,
}

#[derive(Accounts)]
pub struct ClaimReward<'info> {
    #[account(mut, seeds = [ARENA_SEED, player.key().as_ref()], bump = arena.bump, has_one = player)]
    pub arena: Account<'info, Arena>,
    #[account(mut)]
    pub player: Signer<'info>,
    #[account(mut, seeds = [REWARD_MINT_SEED], bump)]
    pub reward_mint: Account<'info, Mint>,
    /// CHECK: PDA mint authority; only used as a CPI signer via seeds.
    #[account(seeds = [REWARD_MINT_AUTH_SEED], bump)]
    pub mint_authority: UncheckedAccount<'info>,
    #[account(
        init_if_needed,
        payer = player,
        associated_token::mint = reward_mint,
        associated_token::authority = player,
    )]
    pub player_ata: Account<'info, TokenAccount>,
    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
    /// Audit fix M-4: anti-replay ledger. Created lazily on first claim;
    /// NEVER delegated, so it survives any undelegation snapshot revert.
    #[account(
        init_if_needed,
        payer = player,
        space = 8 + PlayerStats::INIT_SPACE,
        seeds = [PLAYER_STATS_SEED, player.key().as_ref()],
        bump
    )]
    pub player_stats: Account<'info, PlayerStats>,
}

/// Per-player claim ledger (audit fix M-4). Never delegated: it lives on
/// base only and records the highest claimed round nonce, so a round can
/// never be claimed twice even if the base arena snapshot reverts.
#[account]
#[derive(InitSpace)]
pub struct PlayerStats {
    pub last_claimed_nonce: u64, // 8 — highest round_nonce claimed so far
    pub total_minted: u64,       // 8 — lifetime RUSH minted to this player
    pub bump: u8,                // 1
    // Total: 17 bytes + 8 discriminator = 25 bytes.
}

#[account]
#[derive(InitSpace)]
pub struct Arena {
    pub player: Pubkey,       // 32
    pub score: u32,           // 4
    pub state: u8,            // 1
    pub multiplier: u8,       // 1
    pub pending_nonce: u64,   // 8 — round nonce of the live VRF request
    pub round_nonce: u64,     // 8 — increments per round; request id
    pub last_tap_slot: u64,   // 8
    pub taps_this_slot: u8,   // 1
    pub last_target_id: u16,  // 2
    pub best_score: u32,      // 4 — persistent per-player best (leaderboard)
    pub round_start_ts: i64,  // 8 — unix ts when the round started
    pub vrf_request_ts: i64,  // 8 — unix ts of the live VRF request (0 if none)
    pub bump: u8,             // 1
    // Total: 86 bytes + 8 discriminator = 94 bytes — far under 10,240.
}

#[error_code]
pub enum ArenaError {
    #[msg("Round is not in a startable state")]
    RoundNotStartable,
    #[msg("Round is not in progress")]
    NotPlaying,
    #[msg("Round is not awaiting randomness")]
    NotRolling,
    #[msg("Round is not resolved yet")]
    NotResolved,
    #[msg("Callback is for a superseded request")]
    StaleCallback,
    #[msg("Tap rate limit exceeded for this slot")]
    TapRateLimited,
    #[msg("Target id replayed")]
    TargetReplay,
    #[msg("Round time has expired")]
    RoundTimeExpired,
    #[msg("VRF request has not expired yet")]
    VrfNotExpired,
    // --- Audit-fix errors (appended at the END so existing codes 6000-6008
    // never shift; clients pin these codes in tests). ---
    #[msg("Arena must be IDLE or CLAIMED to delegate")]
    CannotDelegateUnclaimed, // 6009 (H-1)
    #[msg("This round has already been claimed")]
    RoundAlreadyClaimed, // 6010 (M-4)
    #[msg("Round nonce overflow")]
    NonceOverflow, // 6011 (L-5)
}
