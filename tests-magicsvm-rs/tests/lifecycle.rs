//! Full lifecycle test for Arena Rush on MagicSVM 0.3.0.
//!
//! Covers: init_reward_mint → initialize → delegate → start_round → tap →
//! end_round → VRF callback (warp_to_slot) → commit-and-undelegate →
//! claim_reward (SPL mint), plus guard tests and force_expire.

use anchor_lang::InstructionData;
use arena_rush_magicsvm_tests::*;
use magicsvm::{MagicSVM, TransactionTarget};
use solana_address::Address;
use solana_clock::Clock;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_signer::Signer;

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

fn ix_force_expire(arena: &Address, player: &Address) -> Instruction {
    Instruction {
        program_id: PROGRAM_ADDRESS,
        accounts: vec![rw(*arena), signer(*player)],
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
    let base_state = read_arena(&svm, TransactionTarget::Base, &arena);
    assert_eq!(base_state.state, 3, "base RESOLVED after commit");
    assert_eq!(base_state.score, 5);
    assert_eq!(base_state.multiplier, er_state.multiplier);

    // Base: claim the SPL reward.
    send_ix(
        &mut svm,
        TransactionTarget::Base,
        &payer,
        ix_claim(&arena, &player, &mint, &auth, &ata),
    )
    .expect("claim_reward");
    let base_state = read_arena(&svm, TransactionTarget::Base, &arena);
    assert_eq!(base_state.state, 4, "CLAIMED");
    let balance = read_token_balance(&svm, TransactionTarget::Base, &ata);
    assert_eq!(balance, expected_reward, "RUSH minted = score x multiplier");

    // Second claim must fail (already CLAIMED).
    let err = send_ix(
        &mut svm,
        TransactionTarget::Base,
        &payer,
        ix_claim(&arena, &player, &mint, &auth, &ata),
    )
    .expect_err("second claim fails");
    assert!(err.contains("NotResolved") || err.contains("Custom"), "err: {err}");
}

#[test]
fn guards() {
    let (mut svm, payer) = setup_svm();
    let player = payer.pubkey();
    let (arena, _) = arena_pda(&player);
    let (mint, _) = reward_mint_pda();
    let (auth, _) = reward_mint_auth_pda();
    let ata = ata_pda(&player, &mint);

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

    // Tap before start_round -> NotPlaying.
    let err = send_ix(
        &mut svm,
        TransactionTarget::Ephemeral,
        &payer,
        ix_tap(&arena, &player, 1),
    )
    .expect_err("tap before start fails");
    assert!(err.contains("NotPlaying") || err.contains("Custom"), "err: {err}");

    // end_round before start -> NotPlaying.
    let err = send_ix(
        &mut svm,
        TransactionTarget::Ephemeral,
        &payer,
        ix_end_round(&arena, &player),
    )
    .expect_err("end_round before start fails");
    assert!(err.contains("NotPlaying") || err.contains("Custom"), "err: {err}");

    // Commit before resolve -> NotResolved.
    let err = send_ix(
        &mut svm,
        TransactionTarget::Ephemeral,
        &payer,
        ix_commit(&arena, &player),
    )
    .expect_err("commit before resolve fails");
    assert!(err.contains("NotResolved") || err.contains("Custom"), "err: {err}");

    // Claim before resolve -> NotResolved.
    let err = send_ix(
        &mut svm,
        TransactionTarget::Base,
        &payer,
        ix_claim(&arena, &player, &mint, &auth, &ata),
    )
    .expect_err("claim before resolve fails");
    assert!(err.contains("NotResolved") || err.contains("Custom"), "err: {err}");

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
    assert!(err.contains("TargetReplay") || err.contains("Custom"), "err: {err}");

    // force_expire before the VRF timeout -> VrfNotExpired (state PLAYING
    // also fails first with NotRolling; either guard is fine).
    let err = send_ix(
        &mut svm,
        TransactionTarget::Ephemeral,
        &payer,
        ix_force_expire(&arena, &player),
    )
    .expect_err("force_expire while PLAYING fails");
    assert!(
        err.contains("NotRolling") || err.contains("Custom"),
        "err: {err}"
    );
}

#[test]
fn force_expire_recovers_stuck_round() {
    let (mut svm, payer) = setup_svm();
    let player = payer.pubkey();
    let (arena, _) = arena_pda(&player);
    let (mint, _) = reward_mint_pda();
    let (auth, _) = reward_mint_auth_pda();
    let ata = ata_pda(&player, &mint);

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
    // Too early: force_expire must fail.
    let err = send_ix(
        &mut svm,
        TransactionTarget::Ephemeral,
        &payer,
        ix_force_expire(&arena, &player),
    )
    .expect_err("force_expire too early fails");
    assert!(
        err.contains("VrfNotExpired") || err.contains("Custom"),
        "err: {err}"
    );

    // Advance the clock 200 s on both ledgers, then expire.
    let mut clock = svm.ephemeral().get_sysvar::<Clock>();
    clock.unix_timestamp += 200;
    svm.set_sysvar(&clock);
    send_ix(
        &mut svm,
        TransactionTarget::Ephemeral,
        &payer,
        ix_force_expire(&arena, &player),
    )
    .expect("force_expire after timeout");
    let state = read_arena(&svm, TransactionTarget::Ephemeral, &arena);
    assert_eq!(state.state, 3, "RESOLVED via force_expire");
    assert_eq!(state.multiplier, 1, "fallback Common x1");
    assert_eq!(state.score, 2);

    // A late VRF fulfillment afterwards must NOT re-resolve the round:
    // warp past the request slot; the callback should fail (NotRolling)
    // and leave the state untouched.
    let slot = svm.ephemeral().get_sysvar::<Clock>().slot;
    let results = svm.warp_to_slot(slot + 5);
    assert_eq!(results.len(), 1, "one late fulfillment attempted");
    assert!(results[0].is_err(), "late callback rejected");
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
    send_ix(
        &mut svm,
        TransactionTarget::Base,
        &payer,
        ix_claim(&arena, &player, &mint, &auth, &ata),
    )
    .expect("claim after expire");
    let balance = read_token_balance(&svm, TransactionTarget::Base, &ata);
    assert_eq!(balance, 2, "2 RUSH (score 2 x1)");
}
