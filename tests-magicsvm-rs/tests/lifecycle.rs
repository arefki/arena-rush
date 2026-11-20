//! Full lifecycle test for Arena Rush on MagicSVM 0.3.0.
//!
//! Covers: init_reward_mint → initialize → delegate → start_round → tap →
//! end_round → VRF callback (warp_to_slot) → commit-and-undelegate →
//! claim_reward (SPL mint), plus guard tests and force_expire.

use anchor_lang::InstructionData;
use arena_rush::ArenaError;
use arena_rush_magicsvm_tests::*;
use magicsvm::{MagicSVM, TransactionTarget};
use solana_address::Address;
use solana_clock::Clock;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_sysvar::Sysvar;

/// H-3: assert the failure is EXACTLY the given program error code. The old
/// assertions (`err.contains("NotPlaying") || err.contains("Custom")`) were
/// vacuous — every Anchor custom error satisfies `contains("Custom")`, so a
/// wrong guard would still pass. LiteSVM formats the error as
/// `Custom(<code>)` in the Debug output and `custom program error: 0x<code>`
/// in the log lines; either form pins the code.
fn assert_error_code(err: &str, code: u32) {
    let debug_form = format!("Custom({code})");
    let log_form = format!("custom program error: 0x{code:x}");
    assert!(
        err.contains(&debug_form) || err.contains(&log_form),
        "expected error code {code} ({debug_form} / {log_form}), got: {err}"
    );
}

/// Anchor framework error for a failed seeds/bump constraint (used by the
/// cross-player isolation test — the foreign player's PDA does not match).
const CONSTRAINT_SEEDS: u32 = 2006;

/// Anchor framework error when an account is owned by another program
/// (used for claims on a delegated arena — the base copy is owned by the
/// delegation program while delegated).
const ACCOUNT_OWNED_BY_WRONG_PROGRAM: u32 = 3007;

/// Simulate finalized undelegation (MagicSVM 0.3.0 quirk workaround).
///
/// After `commit_and_undelegate`, MagicSVM restores the arena owner but
/// leaves the delegation buffer/record/metadata accounts on base. Its
/// `apply_base_account_state` then RE-DELEGATES the arena on the next
/// base tx that writes it, and a later re-delegate fails because the DLP
/// CPI cannot rewrite the existing buffer (`ExternalAccountDataModified`).
/// On the real chain the DLP closes those accounts when undelegation
/// finalizes (the live proof run re-delegates cleanly), so the test
/// removes them to mirror the chain. Call after every commit.
fn finalize_undelegation(svm: &mut MagicSVM, arena: &Address) {
    for pda in [buffer_pda(arena), record_pda(arena), metadata_pda(arena)] {
        svm.set_account(pda, Default::default())
            .expect("clear delegation account");
    }
}

/// Anchor `#[error_code]` discriminants start at 0; the on-chain error
/// number is 6000 + discriminant. (`ArenaError::X as u32` alone gives the
/// discriminant, NOT the code — that mistake made run 2 fail on every
/// exact-code assertion.)
fn err_code(e: ArenaError) -> u32 {
    6000 + e as u32
}

const TOKEN_PROGRAM_ID: Address =
    Address::from_str_const("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
const ASSOCIATED_TOKEN_PROGRAM_ID: Address =
    Address::from_str_const("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
const RENT_SYSVAR: Address =
    Address::from_str_const("SysvarRent111111111111111111111111111111111");
const SLOT_HASHES: Address =
    Address::from_str_const("SysvarS1otHashes111111111111111111111111111");
const VRF_PROGRAM_ID: Address =
    Address::from_str_const("Vrf1RNUjXmQGjmQrQLvJHs9SNkvDJEsRVFPkfSQUwGz");
const DELEGATION_PROGRAM_ID: Address =
    Address::from_str_const("DELeGGvXpWV2fqJUhqcF5ZSYMS4JTLjteaAMARRSaeSh");
const MAGIC_PROGRAM_ID: Address =
    Address::from_str_const("Magic11111111111111111111111111111111111111");
const MAGIC_CONTEXT_ID: Address =
    Address::from_str_const("MagicContext1111111111111111111111111111111");
const SYSTEM_PROGRAM_ID: Address =
    Address::from_str_const("11111111111111111111111111111111");

fn buffer_pda(arena: &Address) -> Address {
    Address::find_program_address(&[b"buffer", arena.as_ref()], &PROGRAM_ADDRESS).0
}
// NOTE: record + metadata live under the DELEGATION program, not the arena
// program (see the delegate ix in target/idl/arena_rush.json — their `pda`
// program is the delegation_program account). Deriving them under the arena
// program fails with anchor ConstraintSeeds (Custom 2006).
fn record_pda(arena: &Address) -> Address {
    Address::find_program_address(&[b"delegation", arena.as_ref()], &DELEGATION_PROGRAM_ID).0
}
fn metadata_pda(arena: &Address) -> Address {
    Address::find_program_address(
        &[b"delegation-metadata", arena.as_ref()],
        &DELEGATION_PROGRAM_ID,
    )
    .0
}
fn program_identity_pda() -> Address {
    Address::find_program_address(&[b"identity"], &PROGRAM_ADDRESS).0
}
fn ata_pda(owner: &Address, mint: &Address) -> Address {
    Address::find_program_address(
        &[
            owner.as_ref(),
            TOKEN_PROGRAM_ID.as_ref(),
            mint.as_ref(),
        ],
        &ASSOCIATED_TOKEN_PROGRAM_ID,
    )
    .0
}

fn ix_initialize(arena: &Address, player: &Address) -> Instruction {
    Instruction {
        program_id: PROGRAM_ADDRESS,
        accounts: vec![
            rw(*arena),
            signer_rw(*player),
            ro(SYSTEM_PROGRAM_ID),
        ],
        data: instruction::Initialize {}.data(),
    }
}

fn ix_init_reward_mint(mint: &Address, auth: &Address, payer: &Address) -> Instruction {
    Instruction {
        program_id: PROGRAM_ADDRESS,
        accounts: vec![
            rw(*mint),
            ro(*auth),
            signer_rw(*payer),
            ro(TOKEN_PROGRAM_ID),
            ro(SYSTEM_PROGRAM_ID),
            ro(RENT_SYSVAR),
        ],
        data: instruction::InitRewardMint {}.data(),
    }
}

fn ix_delegate(arena: &Address, player: &Address) -> Instruction {
    Instruction {
        program_id: PROGRAM_ADDRESS,
        accounts: vec![
            signer_rw(*player),
            rw(buffer_pda(arena)),
            rw(record_pda(arena)),
            rw(metadata_pda(arena)),
            rw(*arena),
            ro(PROGRAM_ADDRESS),
            ro(DELEGATION_PROGRAM_ID),
            ro(SYSTEM_PROGRAM_ID),
        ],
        data: instruction::Delegate {}.data(),
    }
}

fn ix_start_round(arena: &Address, player: &Address) -> Instruction {
    Instruction {
        program_id: PROGRAM_ADDRESS,
        accounts: vec![rw(*arena), signer(*player)],
        data: instruction::StartRound {}.data(),
    }
}

fn ix_tap(arena: &Address, player: &Address, target_id: u16) -> Instruction {
    Instruction {
        program_id: PROGRAM_ADDRESS,
        accounts: vec![rw(*arena), signer(*player)],
        data: instruction::Tap { target_id }.data(),
    }
}

fn ix_end_round(arena: &Address, player: &Address) -> Instruction {
    Instruction {
        program_id: PROGRAM_ADDRESS,
        accounts: vec![
            signer_rw(*player),
            rw(*arena),
            rw(EPHEMERAL_QUEUE),
            ro(program_identity_pda()),
            ro(VRF_PROGRAM_ID),
            ro(SLOT_HASHES),
            ro(SYSTEM_PROGRAM_ID),
        ],
        data: instruction::EndRound {}.data(),
    }
}

// M-1: force_expire is permissionless — [arena, player (read-only, seeds
// only), caller (signer)]. Self-use passes the player as the caller.
fn ix_force_expire(arena: &Address, player: &Address, caller: &Address) -> Instruction {
    Instruction {
        program_id: PROGRAM_ADDRESS,
        accounts: vec![rw(*arena), ro(*player), signer(*caller)],
        data: instruction::ForceExpire {}.data(),
    }
}

fn ix_commit(arena: &Address, player: &Address) -> Instruction {
    Instruction {
        program_id: PROGRAM_ADDRESS,
        accounts: vec![
            signer_rw(*player),
            rw(*arena),
            ro(MAGIC_PROGRAM_ID),
            rw(MAGIC_CONTEXT_ID),
        ],
        data: instruction::CommitAndUndelegate {}.data(),
    }
}

fn ix_claim(
    arena: &Address,
    player: &Address,
    mint: &Address,
    auth: &Address,
    ata: &Address,
    stats: &Address,
) -> Instruction {
    Instruction {
        program_id: PROGRAM_ADDRESS,
        accounts: vec![
            rw(*arena),
            signer_rw(*player),
            rw(*mint),
            ro(*auth),
            rw(*ata),
            ro(TOKEN_PROGRAM_ID),
            ro(ASSOCIATED_TOKEN_PROGRAM_ID),
            ro(SYSTEM_PROGRAM_ID),
            // M-4: anti-replay ledger (last account in IDL order).
            rw(*stats),
        ],
        data: instruction::ClaimReward {}.data(),
    }
}

/// Play one full round on the ER up to (not incl.) commit. Returns the arena.
fn play_round_to_resolved(
    svm: &mut MagicSVM,
    payer: &Keypair,
    arena: &Address,
    taps: &[(u16,)],
) {
    let player = payer.pubkey();
    send_ix(
        svm,
        TransactionTarget::Ephemeral,
        payer,
        ix_start_round(arena, &player),
    )
    .expect("start_round");
    for (target_id,) in taps {
        warp_one_slot(svm);
        send_ix(
            svm,
            TransactionTarget::Ephemeral,
            payer,
            ix_tap(arena, &player, *target_id),
        )
        .expect("tap");
    }
    let state = read_arena(svm, TransactionTarget::Ephemeral, arena);
    assert_eq!(state.score as usize, taps.len());
    assert_eq!(state.state, 1, "PLAYING");

    // Pin the VRF randomness so the test is deterministic.
    svm.set_vrf_randomness(Some([77u8; 32]));
    send_ix(
        svm,
        TransactionTarget::Ephemeral,
        payer,
        ix_end_round(arena, &player),
    )
    .expect("end_round");
    let state = read_arena(svm, TransactionTarget::Ephemeral, arena);
    assert_eq!(state.state, 2, "ROLLING");

    // VRF fulfills in the slot after the request.
    let slot = svm.ephemeral().get_sysvar::<Clock>().slot;
    let results = svm.warp_to_slot(slot + 1);
    assert_eq!(results.len(), 1, "one VRF fulfillment");
    assert!(results[0].is_ok(), "fulfillment ok: {:?}", results[0]);

    let state = read_arena(svm, TransactionTarget::Ephemeral, arena);
    assert_eq!(state.state, 3, "RESOLVED");
    assert!((1..=5).contains(&state.multiplier), "multiplier tier");
    assert_eq!(state.best_score as usize, taps.len());
}

#[test]
fn full_lifecycle() {
    let (mut svm, payer) = setup_svm();
    let player = payer.pubkey();
    let (arena, _) = arena_pda(&player);
    let (mint, _) = reward_mint_pda();
    let (auth, _) = reward_mint_auth_pda();
    let ata = ata_pda(&player, &mint);
    let (stats, _) = player_stats_pda(&player);

    // Base: reward mint + arena.
    send_ix(
        &mut svm,
        TransactionTarget::Base,
        &payer,
        ix_init_reward_mint(&mint, &auth, &player),
    )
    .expect("init_reward_mint");
    send_ix(
        &mut svm,
        TransactionTarget::Base,
        &payer,
        ix_initialize(&arena, &player),
    )
    .expect("initialize");
    let state = read_arena(&svm, TransactionTarget::Base, &arena);
    assert_eq!(state.state, 0, "IDLE");
    assert_eq!(state.score, 0);

    // Base: delegate to the ER.
    send_ix(
        &mut svm,
        TransactionTarget::Base,
        &payer,
        ix_delegate(&arena, &player),
    )
    .expect("delegate");
    let base_acct = svm
        .get_account_for(TransactionTarget::Base, &arena)
        .expect("arena on base");
    assert_eq!(
        base_acct.owner, DELEGATION_PROGRAM_ID,
        "base owner is delegation program while delegated"
    );

    // ER: play the round.
    play_round_to_resolved(&mut svm, &payer, &arena, &[(1,), (2,), (3,), (4,), (5,)]);
    let er_state = read_arena(&svm, TransactionTarget::Ephemeral, &arena);
    let expected_reward = ((er_state.score as u64) * (er_state.multiplier as u64)).min(1000);

    // ER: commit + undelegate.
    send_ix(
        &mut svm,
        TransactionTarget::Ephemeral,
        &payer,
        ix_commit(&arena, &player),
    )
    .expect("commit_and_undelegate");
    finalize_undelegation(&mut svm, &arena);
    let base_state = read_arena(&svm, TransactionTarget::Base, &arena);
    assert_eq!(base_state.state, 3, "base RESOLVED after commit");
    assert_eq!(base_state.score, 5);
    assert_eq!(base_state.multiplier, er_state.multiplier);

    // Base: claim the SPL reward.
    send_ix(
        &mut svm,
        TransactionTarget::Base,
        &payer,
        ix_claim(&arena, &player, &mint, &auth, &ata, &stats),
    )
    .expect("claim_reward");
    let base_state = read_arena(&svm, TransactionTarget::Base, &arena);
    assert_eq!(base_state.state, 4, "CLAIMED");
    let balance = read_token_balance(&svm, TransactionTarget::Base, &ata);
    assert_eq!(balance, expected_reward, "RUSH minted = score x multiplier");
    // M-4: the anti-replay ledger recorded the claim.
    let ledger = read_player_stats(&svm, TransactionTarget::Base, &stats);
    assert_eq!(ledger.last_claimed_nonce, 1, "round 1 recorded as claimed");
    assert_eq!(ledger.total_minted, expected_reward, "lifetime minted");

    // Second claim must fail (already CLAIMED) — exact code, H-3.
    let err = send_ix(
        &mut svm,
        TransactionTarget::Base,
        &payer,
        ix_claim(&arena, &player, &mint, &auth, &ata, &stats),
    )
    .expect_err("second claim fails");
    assert_error_code(&err, err_code(ArenaError::NotResolved));
}

#[test]
fn guards() {
    let (mut svm, payer) = setup_svm();
    let player = payer.pubkey();
    let (arena, _) = arena_pda(&player);
    let (mint, _) = reward_mint_pda();
    let (auth, _) = reward_mint_auth_pda();
    let ata = ata_pda(&player, &mint);
    let (stats, _) = player_stats_pda(&player);

    send_ix(
        &mut svm,
        TransactionTarget::Base,
        &payer,
        ix_init_reward_mint(&mint, &auth, &player),
    )
    .expect("init_reward_mint");
    send_ix(
        &mut svm,
        TransactionTarget::Base,
        &payer,
        ix_initialize(&arena, &player),
    )
    .expect("initialize");
    send_ix(
        &mut svm,
        TransactionTarget::Base,
        &payer,
        ix_delegate(&arena, &player),
    )
    .expect("delegate");

    // Tap before start_round -> NotPlaying (6001, exact code — H-3).
    let err = send_ix(
        &mut svm,
        TransactionTarget::Ephemeral,
        &payer,
        ix_tap(&arena, &player, 1),
    )
    .expect_err("tap before start fails");
    assert_error_code(&err, err_code(ArenaError::NotPlaying));

    // end_round before start -> NotPlaying (6001).
    let err = send_ix(
        &mut svm,
        TransactionTarget::Ephemeral,
        &payer,
        ix_end_round(&arena, &player),
    )
    .expect_err("end_round before start fails");
    assert_error_code(&err, err_code(ArenaError::NotPlaying));

    // Commit before resolve -> NotResolved (6003).
    let err = send_ix(
        &mut svm,
        TransactionTarget::Ephemeral,
        &payer,
        ix_commit(&arena, &player),
    )
    .expect_err("commit before resolve fails");
    assert_error_code(&err, err_code(ArenaError::NotResolved));

    // Claim while the arena is DELEGATED -> AccountOwnedByWrongProgram
    // (3007): the base copy is owned by the delegation program, so
    // Anchor's owner check fires before the handler's state check. The
    // old test asserted NotResolved here — but the error was ALWAYS 3007
    // and the `|| contains("Custom")` fallback hid it. This is the H-3
    // vacuity made visible. The real NotResolved guard is covered by
    // `claim_not_resolved_undelegated` below (undelegated arena).
    let err = send_ix(
        &mut svm,
        TransactionTarget::Base,
        &payer,
        ix_claim(&arena, &player, &mint, &auth, &ata, &stats),
    )
    .expect_err("claim while delegated fails");
    assert_error_code(&err, ACCOUNT_OWNED_BY_WRONG_PROGRAM);

    // Start, tap, then replay the same target -> TargetReplay.
    send_ix(
        &mut svm,
        TransactionTarget::Ephemeral,
        &payer,
        ix_start_round(&arena, &player),
    )
    .expect("start_round");
    warp_one_slot(&mut svm);
    send_ix(
        &mut svm,
        TransactionTarget::Ephemeral,
        &payer,
        ix_tap(&arena, &player, 7),
    )
    .expect("tap 7");
    // Warp first: without a fresh slot the replay tx is byte-identical to
    // the first tap (AlreadyProcessed) and the slot cap would fire first.
    warp_one_slot(&mut svm);
    let err = send_ix(
        &mut svm,
        TransactionTarget::Ephemeral,
        &payer,
        ix_tap(&arena, &player, 7),
    )
    .expect_err("same target twice fails");
    assert_error_code(&err, err_code(ArenaError::TargetReplay));

    // force_expire while PLAYING -> NotRolling (6002). The state check
    // fires before the timeout check.
    let err = send_ix(
        &mut svm,
        TransactionTarget::Ephemeral,
        &payer,
        ix_force_expire(&arena, &player, &player),
    )
    .expect_err("force_expire while PLAYING fails");
    assert_error_code(&err, err_code(ArenaError::NotRolling));
}

#[test]
fn force_expire_recovers_stuck_round() {
    let (mut svm, payer) = setup_svm();
    let player = payer.pubkey();
    let (arena, _) = arena_pda(&player);
    let (mint, _) = reward_mint_pda();
    let (auth, _) = reward_mint_auth_pda();
    let ata = ata_pda(&player, &mint);
    let (stats, _) = player_stats_pda(&player);

    send_ix(
        &mut svm,
        TransactionTarget::Base,
        &payer,
        ix_init_reward_mint(&mint, &auth, &player),
    )
    .expect("init_reward_mint");
    send_ix(
        &mut svm,
        TransactionTarget::Base,
        &payer,
        ix_initialize(&arena, &player),
    )
    .expect("initialize");
    send_ix(
        &mut svm,
        TransactionTarget::Base,
        &payer,
        ix_delegate(&arena, &player),
    )
    .expect("delegate");
    send_ix(
        &mut svm,
        TransactionTarget::Ephemeral,
        &payer,
        ix_start_round(&arena, &player),
    )
    .expect("start_round");
    warp_one_slot(&mut svm);
    send_ix(
        &mut svm,
        TransactionTarget::Ephemeral,
        &payer,
        ix_tap(&arena, &player, 1),
    )
    .expect("tap");
    warp_one_slot(&mut svm);
    send_ix(
        &mut svm,
        TransactionTarget::Ephemeral,
        &payer,
        ix_tap(&arena, &player, 2),
    )
    .expect("tap");
    send_ix(
        &mut svm,
        TransactionTarget::Ephemeral,
        &payer,
        ix_end_round(&arena, &player),
    )
    .expect("end_round");
    let state = read_arena(&svm, TransactionTarget::Ephemeral, &arena);
    assert_eq!(state.state, 2, "ROLLING");

    // Do NOT warp (no VRF fulfillment — simulates a lost callback).
    // Too early: force_expire must fail with VrfNotExpired (6008, exact — H-3).
    let err = send_ix(
        &mut svm,
        TransactionTarget::Ephemeral,
        &payer,
        ix_force_expire(&arena, &player, &player),
    )
    .expect_err("force_expire too early fails");
    assert_error_code(&err, err_code(ArenaError::VrfNotExpired));

    // Advance the clock 200 s on both ledgers, then expire.
    let mut clock = svm.ephemeral().get_sysvar::<Clock>();
    clock.unix_timestamp += 200;
    svm.set_sysvar(&clock);
    send_ix(
        &mut svm,
        TransactionTarget::Ephemeral,
        &payer,
        ix_force_expire(&arena, &player, &player),
    )
    .expect("force_expire after timeout");
    let state = read_arena(&svm, TransactionTarget::Ephemeral, &arena);
    assert_eq!(state.state, 3, "RESOLVED via force_expire");
    assert_eq!(state.multiplier, 1, "fallback Common x1");
    assert_eq!(state.score, 2);

    // A late VRF fulfillment afterwards must NOT re-resolve the round:
    // warp past the request slot; the callback should fail (NotRolling,
    // 6002 — exact code, H-3) and leave the state untouched.
    let slot = svm.ephemeral().get_sysvar::<Clock>().slot;
    let results = svm.warp_to_slot(slot + 5);
    assert_eq!(results.len(), 1, "one late fulfillment attempted");
    assert!(results[0].is_err(), "late callback rejected");
    assert_error_code(
        &format!("{:?}", results[0]),
        err_code(ArenaError::NotRolling),
    );
    let state = read_arena(&svm, TransactionTarget::Ephemeral, &arena);
    assert_eq!(state.state, 3, "still RESOLVED");
    assert_eq!(state.multiplier, 1, "still fallback x1");

    // Commit + claim still work after expiry.
    send_ix(
        &mut svm,
        TransactionTarget::Ephemeral,
        &payer,
        ix_commit(&arena, &player),
    )
    .expect("commit after expire");
    finalize_undelegation(&mut svm, &arena);
    send_ix(
        &mut svm,
        TransactionTarget::Base,
        &payer,
        ix_claim(&arena, &player, &mint, &auth, &ata, &stats),
    )
    .expect("claim after expire");
    let balance = read_token_balance(&svm, TransactionTarget::Base, &ata);
    assert_eq!(balance, 2, "2 RUSH (score 2 x1)");
}

// ---------- audit-fix tests (H-3 + new negative coverage) ----------

/// Shared setup: reward mint + arena + delegate. Returns everything a
/// round test needs.
struct Fixture {
    svm: MagicSVM,
    payer: Keypair,
    player: Address,
    arena: Address,
    mint: Address,
    auth: Address,
    ata: Address,
    stats: Address,
}

fn setup_delegated() -> Fixture {
    let (mut svm, payer) = setup_svm();
    let player = payer.pubkey();
    let (arena, _) = arena_pda(&player);
    let (mint, _) = reward_mint_pda();
    let (auth, _) = reward_mint_auth_pda();
    let ata = ata_pda(&player, &mint);
    let (stats, _) = player_stats_pda(&player);
    send_ix(
        &mut svm,
        TransactionTarget::Base,
        &payer,
        ix_init_reward_mint(&mint, &auth, &player),
    )
    .expect("init_reward_mint");
    send_ix(
        &mut svm,
        TransactionTarget::Base,
        &payer,
        ix_initialize(&arena, &player),
    )
    .expect("initialize");
    send_ix(
        &mut svm,
        TransactionTarget::Base,
        &payer,
        ix_delegate(&arena, &player),
    )
    .expect("delegate");
    Fixture {
        svm,
        payer,
        player,
        arena,
        mint,
        auth,
        ata,
        stats,
    }
}

/// H-3: MAX_REWARD cap — 201 taps with a pinned Legendary roll (98 -> x5)
/// would pay 1005 RUSH; the claim must mint exactly 1000.
#[test]
fn max_reward_cap() {
    let mut f = setup_delegated();
    send_ix(
        &mut f.svm,
        TransactionTarget::Ephemeral,
        &f.payer,
        ix_start_round(&f.arena, &f.player),
    )
    .expect("start_round");
    // Timestamps do not advance on warp (magicblock-litesvm warp_to_slot
    // moves only the slot), so the 35 s window never trips across 201 taps.
    for i in 0..201u16 {
        warp_one_slot(&mut f.svm);
        send_ix(
            &mut f.svm,
            TransactionTarget::Ephemeral,
            &f.payer,
            ix_tap(&f.arena, &f.player, i),
        )
        .expect("tap");
    }
    let state = read_arena(&f.svm, TransactionTarget::Ephemeral, &f.arena);
    assert_eq!(state.score, 201);
    // Pinned randomness [98u8; 32] -> roll 98 -> Legendary x5.
    f.svm.set_vrf_randomness(Some([98u8; 32]));
    send_ix(
        &mut f.svm,
        TransactionTarget::Ephemeral,
        &f.payer,
        ix_end_round(&f.arena, &f.player),
    )
    .expect("end_round");
    let slot = f.svm.ephemeral().get_sysvar::<Clock>().slot;
    let results = f.svm.warp_to_slot(slot + 1);
    assert_eq!(results.len(), 1, "one VRF fulfillment");
    assert!(results[0].is_ok(), "fulfillment ok: {:?}", results[0]);
    let state = read_arena(&f.svm, TransactionTarget::Ephemeral, &f.arena);
    assert_eq!(state.state, 3, "RESOLVED");
    assert_eq!(state.multiplier, 5, "Legendary x5 from roll 98");
    send_ix(
        &mut f.svm,
        TransactionTarget::Ephemeral,
        &f.payer,
        ix_commit(&f.arena, &f.player),
    )
    .expect("commit");
    finalize_undelegation(&mut f.svm, &f.arena);
    send_ix(
        &mut f.svm,
        TransactionTarget::Base,
        &f.payer,
        ix_claim(&f.arena, &f.player, &f.mint, &f.auth, &f.ata, &f.stats),
    )
    .expect("claim");
    let balance = read_token_balance(&f.svm, TransactionTarget::Base, &f.ata);
    assert_eq!(balance, 1000, "201 x 5 = 1005 capped to MAX_REWARD 1000");
    let ledger = read_player_stats(&f.svm, TransactionTarget::Base, &f.stats);
    assert_eq!(ledger.total_minted, 1000, "ledger records the capped amount");
}

/// H-3: 35 s round window — a tap 40 s after the round start fails with
/// RoundTimeExpired (6007, exact code).
#[test]
fn round_time_window() {
    let mut f = setup_delegated();
    send_ix(
        &mut f.svm,
        TransactionTarget::Ephemeral,
        &f.payer,
        ix_start_round(&f.arena, &f.player),
    )
    .expect("start_round");
    warp_one_slot(&mut f.svm);
    send_ix(
        &mut f.svm,
        TransactionTarget::Ephemeral,
        &f.payer,
        ix_tap(&f.arena, &f.player, 1),
    )
    .expect("tap in window");
    // Advance the clock 40 s past the round start (window is 30 s + 5 s grace).
    let mut clock = f.svm.ephemeral().get_sysvar::<Clock>();
    clock.unix_timestamp += 40;
    f.svm.set_sysvar(&clock);
    warp_one_slot(&mut f.svm);
    let err = send_ix(
        &mut f.svm,
        TransactionTarget::Ephemeral,
        &f.payer,
        ix_tap(&f.arena, &f.player, 2),
    )
    .expect_err("tap after window fails");
    assert_error_code(&err, err_code(ArenaError::RoundTimeExpired));
}

/// H-3: 1-tap-per-slot — two taps in the same slot (different target ids,
/// so the replay guard does not fire first) fail with TapRateLimited (6005).
#[test]
fn one_tap_per_slot() {
    let mut f = setup_delegated();
    send_ix(
        &mut f.svm,
        TransactionTarget::Ephemeral,
        &f.payer,
        ix_start_round(&f.arena, &f.player),
    )
    .expect("start_round");
    send_ix(
        &mut f.svm,
        TransactionTarget::Ephemeral,
        &f.payer,
        ix_tap(&f.arena, &f.player, 1),
    )
    .expect("first tap in slot");
    let err = send_ix(
        &mut f.svm,
        TransactionTarget::Ephemeral,
        &f.payer,
        ix_tap(&f.arena, &f.player, 2),
    )
    .expect_err("second tap in same slot fails");
    assert_error_code(&err, err_code(ArenaError::TapRateLimited));
}

/// H-3: StaleCallback via the natural two-request path — round 1's VRF
/// request is never fulfilled (lost callback), round 1 is force-expired,
/// committed and claimed, round 2 requests fresh randomness, and the warp
/// delivers BOTH callbacks: round 1's must fail StaleCallback (6004) while
/// round 2's resolves the round. No warps happen between the two requests,
/// so round 1's fulfillment is still pending when round 2 requests.
#[test]
fn stale_callback_two_requests() {
    let mut f = setup_delegated();
    // Round 1: start, 1 tap, end_round — NO warp (lost callback).
    send_ix(
        &mut f.svm,
        TransactionTarget::Ephemeral,
        &f.payer,
        ix_start_round(&f.arena, &f.player),
    )
    .expect("start round 1");
    send_ix(
        &mut f.svm,
        TransactionTarget::Ephemeral,
        &f.payer,
        ix_tap(&f.arena, &f.player, 1),
    )
    .expect("tap round 1");
    f.svm.set_vrf_randomness(Some([77u8; 32]));
    send_ix(
        &mut f.svm,
        TransactionTarget::Ephemeral,
        &f.payer,
        ix_end_round(&f.arena, &f.player),
    )
    .expect("end round 1");
    let state = read_arena(&f.svm, TransactionTarget::Ephemeral, &f.arena);
    assert_eq!(state.state, 2, "ROLLING round 1");
    assert_eq!(state.pending_nonce, 1);

    // Force-expire round 1 (clock +200 s), commit, claim.
    let mut clock = f.svm.ephemeral().get_sysvar::<Clock>();
    clock.unix_timestamp += 200;
    f.svm.set_sysvar(&clock);
    send_ix(
        &mut f.svm,
        TransactionTarget::Ephemeral,
        &f.payer,
        ix_force_expire(&f.arena, &f.player, &f.player),
    )
    .expect("force_expire round 1");
    send_ix(
        &mut f.svm,
        TransactionTarget::Ephemeral,
        &f.payer,
        ix_commit(&f.arena, &f.player),
    )
    .expect("commit round 1");
    finalize_undelegation(&mut f.svm, &f.arena);
    send_ix(
        &mut f.svm,
        TransactionTarget::Base,
        &f.payer,
        ix_claim(&f.arena, &f.player, &f.mint, &f.auth, &f.ata, &f.stats),
    )
    .expect("claim round 1");
    assert_eq!(
        read_token_balance(&f.svm, TransactionTarget::Base, &f.ata),
        1,
        "round 1: score 1 x1 fallback"
    );

    // Re-delegate (CLAIMED is delegatable — H-1) and play round 2 with no
    // taps and no warps, so round 1's pending fulfillment is still due.
    send_ix(
        &mut f.svm,
        TransactionTarget::Base,
        &f.payer,
        ix_delegate(&f.arena, &f.player),
    )
    .expect("re-delegate");
    send_ix(
        &mut f.svm,
        TransactionTarget::Ephemeral,
        &f.payer,
        ix_start_round(&f.arena, &f.player),
    )
    .expect("start round 2");
    send_ix(
        &mut f.svm,
        TransactionTarget::Ephemeral,
        &f.payer,
        ix_end_round(&f.arena, &f.player),
    )
    .expect("end round 2");
    let state = read_arena(&f.svm, TransactionTarget::Ephemeral, &f.arena);
    assert_eq!(state.state, 2, "ROLLING round 2");
    assert_eq!(state.pending_nonce, 2);

    // Warp one slot: BOTH fulfillments are due. Round 1's callback runs
    // first (earlier request sequence) and must fail StaleCallback
    // (nonce 1 != pending 2); round 2's callback resolves the round.
    let slot = f.svm.ephemeral().get_sysvar::<Clock>().slot;
    let results = f.svm.warp_to_slot(slot + 1);
    assert_eq!(results.len(), 2, "both fulfillments attempted");
    assert!(results[0].is_err(), "round 1 callback rejected");
    assert_error_code(
        &format!("{:?}", results[0]),
        err_code(ArenaError::StaleCallback),
    );
    assert!(results[1].is_ok(), "round 2 callback ok: {:?}", results[1]);
    let state = read_arena(&f.svm, TransactionTarget::Ephemeral, &f.arena);
    assert_eq!(state.state, 3, "RESOLVED round 2");
    assert_eq!(state.multiplier, 2, "Rare x2 from pinned roll 77");
    assert_eq!(state.score, 0, "round 2 had no taps");
}

/// H-3: RoundNotStartable (6000) from PLAYING, ROLLING and RESOLVED.
#[test]
fn round_not_startable() {
    let mut f = setup_delegated();
    send_ix(
        &mut f.svm,
        TransactionTarget::Ephemeral,
        &f.payer,
        ix_start_round(&f.arena, &f.player),
    )
    .expect("start round 1");
    let err = send_ix(
        &mut f.svm,
        TransactionTarget::Ephemeral,
        &f.payer,
        ix_start_round(&f.arena, &f.player),
    )
    .expect_err("start while PLAYING fails");
    assert_error_code(&err, err_code(ArenaError::RoundNotStartable));

    f.svm.set_vrf_randomness(Some([77u8; 32]));
    send_ix(
        &mut f.svm,
        TransactionTarget::Ephemeral,
        &f.payer,
        ix_end_round(&f.arena, &f.player),
    )
    .expect("end_round");
    let err = send_ix(
        &mut f.svm,
        TransactionTarget::Ephemeral,
        &f.payer,
        ix_start_round(&f.arena, &f.player),
    )
    .expect_err("start while ROLLING fails");
    assert_error_code(&err, err_code(ArenaError::RoundNotStartable));

    let slot = f.svm.ephemeral().get_sysvar::<Clock>().slot;
    let results = f.svm.warp_to_slot(slot + 1);
    assert_eq!(results.len(), 1, "one VRF fulfillment");
    assert!(results[0].is_ok(), "fulfillment ok: {:?}", results[0]);
    let state = read_arena(&f.svm, TransactionTarget::Ephemeral, &f.arena);
    assert_eq!(state.state, 3, "RESOLVED");
    let err = send_ix(
        &mut f.svm,
        TransactionTarget::Ephemeral,
        &f.payer,
        ix_start_round(&f.arena, &f.player),
    )
    .expect_err("start while RESOLVED fails");
    assert_error_code(&err, err_code(ArenaError::RoundNotStartable));
}

/// H-3: cross-player isolation — a foreign signer cannot tap, end, or
/// claim another player's arena (Anchor ConstraintSeeds, 2006).
#[test]
fn cross_player_isolation() {
    let mut f = setup_delegated();
    let other = Keypair::new();
    let other_pk = other.pubkey();
    // The foreign player pays for their own txs: on the ER, a writable
    // non-fee-payer with no ephemeral account is rejected before the
    // program runs (MagicSVM InvalidWritableAccount), which would mask
    // the 2006 seeds error this test pins.
    f.svm
        .airdrop(&other_pk, 1_000_000_000)
        .expect("airdrop other");
    send_ix(
        &mut f.svm,
        TransactionTarget::Ephemeral,
        &f.payer,
        ix_start_round(&f.arena, &f.player),
    )
    .expect("start");
    // Foreign tap on player A's arena.
    let err = send_ix(
        &mut f.svm,
        TransactionTarget::Ephemeral,
        &other,
        ix_tap(&f.arena, &other_pk, 1),
    )
    .expect_err("foreign tap fails");
    assert_error_code(&err, CONSTRAINT_SEEDS);
    // Foreign end_round.
    let err = send_ix(
        &mut f.svm,
        TransactionTarget::Ephemeral,
        &other,
        ix_end_round(&f.arena, &other_pk),
    )
    .expect_err("foreign end_round fails");
    assert_error_code(&err, CONSTRAINT_SEEDS);

    // Play out the round as A, commit to base.
    warp_one_slot(&mut f.svm);
    send_ix(
        &mut f.svm,
        TransactionTarget::Ephemeral,
        &f.payer,
        ix_tap(&f.arena, &f.player, 1),
    )
    .expect("tap");
    f.svm.set_vrf_randomness(Some([77u8; 32]));
    send_ix(
        &mut f.svm,
        TransactionTarget::Ephemeral,
        &f.payer,
        ix_end_round(&f.arena, &f.player),
    )
    .expect("end_round");
    let slot = f.svm.ephemeral().get_sysvar::<Clock>().slot;
    let results = f.svm.warp_to_slot(slot + 1);
    assert_eq!(results.len(), 1, "one VRF fulfillment");
    assert!(results[0].is_ok(), "fulfillment ok: {:?}", results[0]);
    send_ix(
        &mut f.svm,
        TransactionTarget::Ephemeral,
        &f.payer,
        ix_commit(&f.arena, &f.player),
    )
    .expect("commit");
    finalize_undelegation(&mut f.svm, &f.arena);
    // Foreign claim: the arena seeds fail before any mint.
    let other_ata = ata_pda(&other_pk, &f.mint);
    let (other_stats, _) = player_stats_pda(&other_pk);
    let err = send_ix_multi(
        &mut f.svm,
        TransactionTarget::Base,
        &f.payer,
        &[&other],
        ix_claim(
            &f.arena,
            &other_pk,
            &f.mint,
            &f.auth,
            &other_ata,
            &other_stats,
        ),
    )
    .expect_err("foreign claim fails");
    assert_error_code(&err, CONSTRAINT_SEEDS);
    // A's own claim still works.
    send_ix(
        &mut f.svm,
        TransactionTarget::Base,
        &f.payer,
        ix_claim(&f.arena, &f.player, &f.mint, &f.auth, &f.ata, &f.stats),
    )
    .expect("own claim works");
    assert_eq!(
        read_token_balance(&f.svm, TransactionTarget::Base, &f.ata),
        2,
        "score 1 x2 Rare"
    );
}

/// H-1: delegating a RESOLVED arena fails with CannotDelegateUnclaimed
/// (6009); after claiming, delegation works again and round 2 plays.
#[test]
fn delegate_guard_h1() {
    let mut f = setup_delegated();
    play_round_to_resolved(&mut f.svm, &f.payer, &f.arena, &[(1,), (2,)]);
    send_ix(
        &mut f.svm,
        TransactionTarget::Ephemeral,
        &f.payer,
        ix_commit(&f.arena, &f.player),
    )
    .expect("commit");
    finalize_undelegation(&mut f.svm, &f.arena);
    let base_state = read_arena(&f.svm, TransactionTarget::Base, &f.arena);
    assert_eq!(base_state.state, 3, "RESOLVED on base");
    let err = send_ix(
        &mut f.svm,
        TransactionTarget::Base,
        &f.payer,
        ix_delegate(&f.arena, &f.player),
    )
    .expect_err("delegate RESOLVED fails");
    assert_error_code(&err, err_code(ArenaError::CannotDelegateUnclaimed));
    send_ix(
        &mut f.svm,
        TransactionTarget::Base,
        &f.payer,
        ix_claim(&f.arena, &f.player, &f.mint, &f.auth, &f.ata, &f.stats),
    )
    .expect("claim");
    send_ix(
        &mut f.svm,
        TransactionTarget::Base,
        &f.payer,
        ix_delegate(&f.arena, &f.player),
    )
    .expect("delegate after claim");
    send_ix(
        &mut f.svm,
        TransactionTarget::Ephemeral,
        &f.payer,
        ix_start_round(&f.arena, &f.player),
    )
    .expect("start round 2");
    let state = read_arena(&f.svm, TransactionTarget::Ephemeral, &f.arena);
    assert_eq!(state.state, 1, "PLAYING round 2");
    assert_eq!(state.round_nonce, 2);
}

/// M-1: force_expire is permissionless — a stranger with no stake in the
/// round can expire another player's stuck ROLLING arena.
#[test]
fn force_expire_permissionless_m1() {
    let mut f = setup_delegated();
    let stranger = Keypair::new();
    f.svm
        .airdrop(&stranger.pubkey(), 1_000_000_000)
        .expect("airdrop stranger");
    send_ix(
        &mut f.svm,
        TransactionTarget::Ephemeral,
        &f.payer,
        ix_start_round(&f.arena, &f.player),
    )
    .expect("start");
    send_ix(
        &mut f.svm,
        TransactionTarget::Ephemeral,
        &f.payer,
        ix_tap(&f.arena, &f.player, 1),
    )
    .expect("tap");
    send_ix(
        &mut f.svm,
        TransactionTarget::Ephemeral,
        &f.payer,
        ix_end_round(&f.arena, &f.player),
    )
    .expect("end_round");
    let mut clock = f.svm.ephemeral().get_sysvar::<Clock>();
    clock.unix_timestamp += 200;
    f.svm.set_sysvar(&clock);
    // Fee payer = stranger, player account is read-only.
    send_ix(
        &mut f.svm,
        TransactionTarget::Ephemeral,
        &stranger,
        ix_force_expire(&f.arena, &f.player, &stranger.pubkey()),
    )
    .expect("stranger force_expire");
    let state = read_arena(&f.svm, TransactionTarget::Ephemeral, &f.arena);
    assert_eq!(state.state, 3, "RESOLVED via stranger force_expire");
    assert_eq!(state.multiplier, 1, "fallback Common x1");
}

/// M-4: claim replay is blocked by the stats ledger even if the base arena
/// snapshot reverts to RESOLVED (simulated via set_account).
#[test]
fn claim_replay_blocked_m4() {
    let mut f = setup_delegated();
    play_round_to_resolved(&mut f.svm, &f.payer, &f.arena, &[(1,), (2,), (3,)]);
    send_ix(
        &mut f.svm,
        TransactionTarget::Ephemeral,
        &f.payer,
        ix_commit(&f.arena, &f.player),
    )
    .expect("commit");
    finalize_undelegation(&mut f.svm, &f.arena);
    send_ix(
        &mut f.svm,
        TransactionTarget::Base,
        &f.payer,
        ix_claim(&f.arena, &f.player, &f.mint, &f.auth, &f.ata, &f.stats),
    )
    .expect("claim");
    assert_eq!(
        read_token_balance(&f.svm, TransactionTarget::Base, &f.ata),
        6,
        "score 3 x2 Rare"
    );
    let ledger = read_player_stats(&f.svm, TransactionTarget::Base, &f.stats);
    assert_eq!(ledger.last_claimed_nonce, 1);

    // Simulate an undelegation snapshot revert: forge the base arena back
    // to RESOLVED (state byte at offset 44) via set_account. The state
    // check now passes — only the stats ledger blocks the double claim.
    let mut account = f.svm.get_account(&f.arena).expect("arena on base");
    assert_eq!(account.data[44], 4, "CLAIMED before revert");
    account.data[44] = 3; // RESOLVED
    f.svm
        .set_account(f.arena, account)
        .expect("set_account revert");
    let reverted = read_arena(&f.svm, TransactionTarget::Base, &f.arena);
    assert_eq!(reverted.state, 3, "reverted to RESOLVED");

    let err = send_ix(
        &mut f.svm,
        TransactionTarget::Base,
        &f.payer,
        ix_claim(&f.arena, &f.player, &f.mint, &f.auth, &f.ata, &f.stats),
    )
    .expect_err("replay claim fails");
    assert_error_code(&err, err_code(ArenaError::RoundAlreadyClaimed));
    assert_eq!(
        read_token_balance(&f.svm, TransactionTarget::Base, &f.ata),
        6,
        "no double mint"
    );
}

/// H-3: claim on an UNDELEGATED arena that never resolved fails with
/// NotResolved (6003) — the actual state guard, reachable only when the
/// owner check passes (contrast with the delegated claim in `guards`,
/// which fails earlier with 3007).
#[test]
fn claim_not_resolved_undelegated() {
    let (mut svm, payer) = setup_svm();
    let player = payer.pubkey();
    let (arena, _) = arena_pda(&player);
    let (mint, _) = reward_mint_pda();
    let (auth, _) = reward_mint_auth_pda();
    let ata = ata_pda(&player, &mint);
    let (stats, _) = player_stats_pda(&player);
    send_ix(
        &mut svm,
        TransactionTarget::Base,
        &payer,
        ix_init_reward_mint(&mint, &auth, &player),
    )
    .expect("init_reward_mint");
    send_ix(
        &mut svm,
        TransactionTarget::Base,
        &payer,
        ix_initialize(&arena, &player),
    )
    .expect("initialize");
    // No delegate — the arena sits IDLE on base, program-owned.
    let err = send_ix(
        &mut svm,
        TransactionTarget::Base,
        &payer,
        ix_claim(&arena, &player, &mint, &auth, &ata, &stats),
    )
    .expect_err("claim IDLE fails");
    assert_error_code(&err, err_code(ArenaError::NotResolved));
}
