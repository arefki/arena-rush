//! MagicSVM test helpers for Arena Rush.
//!
//! The integration tests in `tests/` drive the full lifecycle against the
//! in-process base + ephemeral ledgers: initialize → delegate → start_round
//! → tap → end_round → VRF callback (via `warp_to_slot`) →
//! commit-and-undelegate → claim_reward (SPL mint).
//!
//! Instruction data comes from the program crate's generated
//! `arena_rush::instruction::*` builders; account metas are listed by hand
//! in IDL order (see `target/idl/arena_rush.json`).

use anchor_lang::AccountDeserialize;
use arena_rush::{Arena, PlayerStats, ID as PROGRAM_ID};
use magicsvm::{MagicSVM, TransactionTarget};
use solana_address::Address;
use solana_clock::Clock;
use solana_hash::Hash;
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_message::Message;
use solana_signature::Signature;
use solana_signer::Signer;
use solana_system_interface::program as system_program;
use solana_sysvar::Sysvar;
use solana_transaction::Transaction;

/// Devnet program id (also the id used in MagicSVM).
pub const PROGRAM_ADDRESS: Address = Address::from_str_const(
    "HDAvFeo4tpoGzmZqvAhRzmYVdF1qi4CCJxQNZjhQj9D2",
);

/// Delegated VRF queue (fee-exempt inside the ER; MagicSVM models it).
pub const EPHEMERAL_QUEUE: Address =
    Address::from_str_const("5hBR571xnXppuCPveTrctfTU7tJLSN94nq7kv7FRK5Tc");

/// Path to the compiled program (relative to the workspace root).
pub const PROGRAM_SO: &str = "../target/deploy/arena_rush.so";

/// Arena PDA for a player.
pub fn arena_pda(player: &Address) -> (Address, u8) {
    Address::find_program_address(&[b"arena", player.as_ref()], &PROGRAM_ADDRESS)
}

/// Reward mint PDA.
pub fn reward_mint_pda() -> (Address, u8) {
    Address::find_program_address(&[b"reward_mint"], &PROGRAM_ADDRESS)
}

/// Reward mint authority PDA.
pub fn reward_mint_auth_pda() -> (Address, u8) {
    Address::find_program_address(&[b"reward_mint_auth"], &PROGRAM_ADDRESS)
}

/// Player-stats (anti-replay ledger) PDA for a player (audit fix M-4).
pub fn player_stats_pda(player: &Address) -> (Address, u8) {
    Address::find_program_address(&[b"player_stats", player.as_ref()], &PROGRAM_ADDRESS)
}

/// Set up a fresh SVM with the program loaded and the payer funded.
pub fn setup_svm() -> (MagicSVM, Keypair) {
    let mut svm = MagicSVM::new();
    svm.add_program_from_file(PROGRAM_ADDRESS, PROGRAM_SO)
        .expect("program .so loads (run anchor build first)");
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 10_000_000_000)
        .expect("airdrop");
    (svm, payer)
}

/// Send one instruction to the given ledger, signed by `payer`.
pub fn send_ix(
    svm: &mut MagicSVM,
    target: TransactionTarget,
    payer: &Keypair,
    ix: Instruction,
) -> Result<(), String> {
    send_ix_multi(svm, target, payer, &[], ix)
}

/// Send one instruction signed by the fee payer PLUS extra signers (e.g. a
/// foreign player keypair for cross-player isolation tests).
pub fn send_ix_multi(
    svm: &mut MagicSVM,
    target: TransactionTarget,
    payer: &Keypair,
    extra_signers: &[&Keypair],
    ix: Instruction,
) -> Result<(), String> {
    // Fresh blockhash per tx: MagicSVM/LiteSVM does NOT rotate the blockhash
    // on warp, so an identical retry (replay tap, force_expire after a clock
    // jump) would be deduped as AlreadyProcessed. Real slots rotate the
    // blockhash, so this mirrors the chain.
    svm.expire_blockhash_for(target);
    let blockhash: Hash = svm.latest_blockhash_for(target);
    let msg = Message::new_with_blockhash(&[ix], Some(&payer.pubkey()), &blockhash);
    let mut signers: Vec<&Keypair> = vec![payer];
    signers.extend_from_slice(extra_signers);
    let tx = Transaction::new(&signers, msg, blockhash);
    svm.send_transaction_to(target, tx)
        .map(|_| ())
        .map_err(|e| format!("{:?} | logs: {:?}", e.err, e.meta.logs))
}

/// Advance both ledgers by one slot. MagicSVM does NOT auto-advance slots
/// between transactions, so consecutive taps would share a slot (tripping the
/// program's 1-tap-per-slot cap) and a blockhash (the SVM dedupes identical
/// transactions with AlreadyProcessed). Warping gives each tap a fresh slot
/// and a fresh blockhash — the same thing the real ER's 10 ms slots do.
pub fn warp_one_slot(svm: &mut MagicSVM) {
    let slot = svm.ephemeral().get_sysvar::<Clock>().slot;
    let results = svm.warp_to_slot(slot + 1);
    assert!(
        results.is_empty(),
        "unexpected due work while warping: {results:?}"
    );
}

/// Read and deserialize the Arena account from a ledger.
pub fn read_arena(svm: &MagicSVM, target: TransactionTarget, arena: &Address) -> Arena {
    let account = svm
        .get_account_for(target, arena)
        .expect("arena account exists");
    let mut data: &[u8] = &account.data;
    Arena::try_deserialize(&mut data).expect("arena deserializes")
}

/// Read and deserialize the PlayerStats account from a ledger.
pub fn read_player_stats(
    svm: &MagicSVM,
    target: TransactionTarget,
    stats: &Address,
) -> PlayerStats {
    let account = svm
        .get_account_for(target, stats)
        .expect("player_stats account exists");
    let mut data: &[u8] = &account.data;
    PlayerStats::try_deserialize(&mut data).expect("player_stats deserializes")
}

/// Read a token account balance (raw amount) from a ledger.
pub fn read_token_balance(svm: &MagicSVM, target: TransactionTarget, ata: &Address) -> u64 {
    let account = svm
        .get_account_for(target, ata)
        .expect("token account exists");
    // SPL TokenAccount layout: mint(32) owner(32) amount(8) ...
    let amount = &account.data[64..72];
    u64::from_le_bytes(amount.try_into().unwrap())
}

/// Re-exported so tests can build instructions without extra imports.
pub use arena_rush::instruction;
pub use solana_instruction::AccountMeta as Meta;

pub fn ro(pubkey: Address) -> AccountMeta {
    AccountMeta::new_readonly(pubkey, false)
}

pub fn rw(pubkey: Address) -> AccountMeta {
    AccountMeta::new(pubkey, false)
}

pub fn signer(pubkey: Address) -> AccountMeta {
    AccountMeta::new_readonly(pubkey, true)
}

pub fn signer_rw(pubkey: Address) -> AccountMeta {
    AccountMeta::new(pubkey, true)
}

pub fn system_program_id() -> Address {
    system_program::id()
}

/// Unused-import guard: keep the program id import honest.
#[allow(dead_code)]
fn _program_id_check() {
    assert_eq!(PROGRAM_ID.to_bytes(), PROGRAM_ADDRESS.to_bytes());
}

/// Unused-import guard for Signature (used by callers via re-export).
#[allow(dead_code)]
type _Sig = Signature;
