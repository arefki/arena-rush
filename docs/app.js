// ARENA RUSH — vanilla JS client (no Anchor TS client, no TS SDK).
// Instructions are hand-encoded from the program IDL:
//   8-byte discriminator + borsh args, account metas in IDL order.
// Devnet only. Demo-wallet mode is localhost-only (or an explicit
// keypair-file pick); it never touches a mainnet or real-user key.
//
// Flow: initialize (base) -> delegate (base) -> start_round (ER) ->
// taps (ER, gasless) -> end_round (ER, VRF request) -> VRF callback
// -> commit_and_undelegate (ER) -> claim_reward (base, SPL mint).
"use strict";
const { Connection, PublicKey, Transaction, TransactionInstruction,
        SystemProgram, Keypair } = solanaWeb3;

// ---------- constants ----------
const PROGRAM_ID = new PublicKey("HDAvFeo4tpoGzmZqvAhRzmYVdF1qi4CCJxQNZjhQj9D2");
const DELEGATION_PROGRAM_ID = new PublicKey("DELeGGvXpWV2fqJUhqcF5ZSYMS4JTLjteaAMARRSaeSh");
const MAGIC_PROGRAM_ID = new PublicKey("Magic11111111111111111111111111111111111111");
const MAGIC_CONTEXT_ID = new PublicKey("MagicContext1111111111111111111111111111111");
const VRF_PROGRAM_ID = new PublicKey("Vrf1RNUjXmQGjmQrQLvJHs9SNkvDJEsRVFPkfSQUwGz");
const EPHEMERAL_QUEUE = new PublicKey("5hBR571xnXppuCPveTrctfTU7tJLSN94nq7kv7FRK5Tc");
const TOKEN_PROGRAM_ID = new PublicKey("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
const ASSOCIATED_TOKEN_PROGRAM_ID = new PublicKey("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
const SLOT_HASHES = new PublicKey("SysvarS1otHashes111111111111111111111111111");
const RENT_SYSVAR = new PublicKey("SysvarRent111111111111111111111111111111111");
const BASE_RPC = "https://rpc.magicblock.app/devnet";
const ROUTER_RPC = "https://devnet-router.magicblock.app/";
const DEFAULT_ER = "https://devnet-as.magicblock.app/";
const ROUND_SECONDS = 30, ROUND_GRACE = 5, VRF_TIMEOUT_S = 130, MAX_REWARD = 1000;
const STATE_NAMES = ["IDLE", "PLAYING", "ROLLING", "RESOLVED", "CLAIMED"];
const TIERS = { 1: ["Common", "common", "⚪"], 2: ["Rare", "rare", "🔵"],
                3: ["Epic", "epic", "🟣"], 5: ["Legendary", "legendary", "🌟"] };

// Instruction discriminators (from target/idl/arena_rush.json).
const DISC = {
  initialize: [175, 175, 109, 31, 13, 152, 155, 237],
  init_reward_mint: [7, 81, 73, 12, 174, 180, 120, 165],
  delegate: [90, 147, 75, 178, 85, 88, 4, 137],
  start_round: [144, 144, 43, 7, 193, 42, 217, 215],
  tap: [31, 254, 225, 122, 3, 186, 67, 245],
  end_round: [54, 47, 1, 200, 250, 6, 144, 63],
  force_expire: [181, 233, 225, 150, 213, 57, 145, 169],
  commit_and_undelegate: [9, 108, 132, 87, 184, 76, 98, 84],
  claim_reward: [149, 95, 181, 242, 94, 90, 158, 162],
};

// ---------- state ----------
const baseConn = new Connection(BASE_RPC, "confirmed");
let erConn = new Connection(DEFAULT_ER, "confirmed");
let wallet = null;          // { publicKey, signTransaction(tx) }
let demoMode = false;
let pdas = null;            // { arena, mint, auth, buffer, record, metadata, identity, ata, stats }
let arenaState = null;      // decoded arena account (latest poll)
let lobbyAction = "enter";  // H-2: enter | claim | commit-claim | resume | check-vrf
let roundTimer = null, pollTimer = null;
let lastTargetId = 0xffff, pendingTaps = 0, confirmedTaps = 0;
let roundActive = false, endingRound = false;

// ---------- helpers ----------
const $ = (id) => document.getElementById(id);
const short = (pk) => pk.toBase58().slice(0, 4) + "…" + pk.toBase58().slice(-4);
const explorerTx = (sig) => `https://explorer.solana.com/tx/${sig}?cluster=devnet`;
function log(el, msg, cls) {
  const d = document.createElement("div");
  if (cls) d.className = cls;
  d.innerHTML = msg;
  el.appendChild(d);
  el.scrollTop = el.scrollHeight;
}
const logLobby = (m, c) => log($("lobby-log"), m, c);
const logChest = (m, c) => log($("chest-log"), m, c);
// L-1: escape untrusted strings (RPC/wallet error text) before they reach
// innerHTML — the log templates themselves are trusted static HTML.
function esc(s) {
  return String(s).replace(/[&<>"']/g,
    (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
}
function showScreen(id) {
  for (const s of document.querySelectorAll(".screen")) s.classList.remove("active");
  $(id).classList.add("active");
}
function addProof(label, sig) {
  // L-1/L-8: DOM-node construction (no innerHTML sink); the `proofs` array
  // is gone — rendering is DOM-only, nothing ever read it back.
  const li = document.createElement("li");
  const b = document.createElement("b");
  b.textContent = label;
  const a = document.createElement("a");
  a.href = explorerTx(sig);
  a.target = "_blank"; a.rel = "noopener";
  a.textContent = sig;
  li.append(b, document.createElement("br"), a);
  $("proof-links").appendChild(li);
}
async function findPda(seeds, programId) {
  const [pk] = await PublicKey.findProgramAddress(seeds, programId);
  return pk;
}
async function computePdas(player) {
  const arena = await findPda([new TextEncoder().encode("arena"), player.toBuffer()], PROGRAM_ID);
  const mint = await findPda([new TextEncoder().encode("reward_mint")], PROGRAM_ID);
  const auth = await findPda([new TextEncoder().encode("reward_mint_auth")], PROGRAM_ID);
  const buffer = await findPda([new TextEncoder().encode("buffer"), arena.toBuffer()], PROGRAM_ID);
  // NOTE: record + metadata live under the DELEGATION program, not the arena
  // program (their IDL `pda` program is the delegation_program account).
  const record = await findPda([new TextEncoder().encode("delegation"), arena.toBuffer()], DELEGATION_PROGRAM_ID);
  const metadata = await findPda([new TextEncoder().encode("delegation-metadata"), arena.toBuffer()], DELEGATION_PROGRAM_ID);
  const identity = await findPda([new TextEncoder().encode("identity")], PROGRAM_ID);
  const ata = await findPda([player.toBuffer(), TOKEN_PROGRAM_ID.toBuffer(), mint.toBuffer()],
                            ASSOCIATED_TOKEN_PROGRAM_ID);
  // M-4: anti-replay ledger PDA (never delegated, lives on base only).
  const stats = await findPda([new TextEncoder().encode("player_stats"), player.toBuffer()], PROGRAM_ID);
  return { arena, mint, auth, buffer, record, metadata, identity, ata, stats };
}

// Arena layout (94 bytes): disc 8 | player 32 | score u32 | state u8 |
// multiplier u8 | pending_nonce u64 | round_nonce u64 | last_tap_slot u64 |
// taps_this_slot u8 | last_target_id u16 | best_score u32 |
// round_start_ts i64 | vrf_request_ts i64 | bump u8
function decodeArena(data) {
  const dv = new DataView(data.buffer, data.byteOffset, data.byteLength);
  return {
    player: new PublicKey(data.slice(8, 40)),
    score: dv.getUint32(40, true),
    state: dv.getUint8(44),
    multiplier: dv.getUint8(45),
    pendingNonce: dv.getBigUint64(46, true),
    roundNonce: dv.getBigUint64(54, true),
    lastTapSlot: dv.getBigUint64(62, true),
    tapsThisSlot: dv.getUint8(70),
    lastTargetId: dv.getUint16(71, true),
    bestScore: dv.getUint32(73, true),
    roundStartTs: dv.getBigInt64(77, true),
    vrfRequestTs: dv.getBigInt64(85, true),
    bump: dv.getUint8(93),
  };
}
async function fetchArena(conn) {
  const info = await conn.getAccountInfo(pdas.arena);
  if (!info) return null;
  return decodeArena(info.data);
}

// ---------- instruction builders (metas in IDL order) ----------
const ro = (pk) => ({ pubkey: pk, isSigner: false, isWritable: false });
const rw = (pk) => ({ pubkey: pk, isSigner: false, isWritable: true });
const sg = (pk) => ({ pubkey: pk, isSigner: true, isWritable: false });
const sgw = (pk) => ({ pubkey: pk, isSigner: true, isWritable: true });
function ix(name, keys, args) {
  const data = args ? new Uint8Array([...DISC[name], ...args]) : new Uint8Array(DISC[name]);
  return new TransactionInstruction({ programId: PROGRAM_ID, keys, data });
}
function ixInitialize(player) {
  return ix("initialize", [rw(pdas.arena), sgw(player), ro(SystemProgram.programId)]);
}
function ixInitRewardMint(player) {
  return ix("init_reward_mint", [rw(pdas.mint), ro(pdas.auth), sgw(player),
    ro(TOKEN_PROGRAM_ID), ro(SystemProgram.programId), ro(RENT_SYSVAR)]);
}
function ixDelegate(player) {
  return ix("delegate", [sgw(player), rw(pdas.buffer), rw(pdas.record), rw(pdas.metadata),
    rw(pdas.arena), ro(PROGRAM_ID), ro(DELEGATION_PROGRAM_ID), ro(SystemProgram.programId)]);
}
function ixStartRound(player) { return ix("start_round", [rw(pdas.arena), sg(player)]); }
function ixTap(player, targetId) {
  const a = new Uint8Array(2); new DataView(a.buffer).setUint16(0, targetId, true);
  return ix("tap", [rw(pdas.arena), sg(player)], a);
}
function ixEndRound(player) {
  return ix("end_round", [sgw(player), rw(pdas.arena), rw(EPHEMERAL_QUEUE),
    ro(pdas.identity), ro(VRF_PROGRAM_ID), ro(SLOT_HASHES), ro(SystemProgram.programId)]);
}
// M-1: force_expire is permissionless — [arena, player (read-only, seeds
// only), caller (signer)]. Self-use passes the wallet as both.
function ixForceExpire(player) { return ix("force_expire", [rw(pdas.arena), ro(player), sg(player)]); }
function ixCommit(player) {
  return ix("commit_and_undelegate", [sgw(player), rw(pdas.arena),
    ro(MAGIC_PROGRAM_ID), rw(MAGIC_CONTEXT_ID)]);
}
function ixClaim(player) {
  // M-4: player_stats ledger appended (last account in IDL order).
  return ix("claim_reward", [rw(pdas.arena), sgw(player), rw(pdas.mint), ro(pdas.auth),
    rw(pdas.ata), ro(TOKEN_PROGRAM_ID), ro(ASSOCIATED_TOKEN_PROGRAM_ID), ro(SystemProgram.programId),
    rw(pdas.stats)]);
}

// ---------- send helpers ----------
async function sendAndConfirm(conn, instruction, label) {
  const { blockhash, lastValidBlockHeight } = await conn.getLatestBlockhash("confirmed");
  const tx = new Transaction({ feePayer: wallet.publicKey, blockhash, lastValidBlockHeight });
  tx.add(instruction);
  const signed = await wallet.signTransaction(tx);
  const sig = await conn.sendRawTransaction(signed.serialize(), { skipPreflight: false });
  await conn.confirmTransaction({ signature: sig, blockhash, lastValidBlockHeight }, "confirmed");
  return sig;
}
// Fire-and-forget tap: confirm in the background, never block the UI.
async function sendTapFireAndForget(targetId) {
  pendingTaps++; updateTapCounts();
  try {
    const sig = await sendAndConfirm(erConn, ixTap(wallet.publicKey, targetId), "tap");
    confirmedTaps++;
    return sig;
  } catch (e) {
    console.warn("tap failed (score reconciles from chain):", e);
  } finally {
    pendingTaps--; updateTapCounts();
  }
  return null;
}
function updateTapCounts() {
  $("confirmed-taps").textContent = confirmedTaps;
  $("pending-taps").textContent = pendingTaps;
}

// Router: which ER owns this delegated account?
async function getDelegationStatus(pubkey) {
  const res = await fetch(ROUTER_RPC, {
    method: "POST", headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "getDelegationStatus",
                           params: { pubkey: pubkey.toBase58() } }),
  });
  const j = await res.json();
  return j.result || null;
}
async function refreshErEndpoint() {
  try {
    const st = await getDelegationStatus(pdas.arena);
    if (st && st.fqdn) {
      // Live router returns fqdn as a FULL URL ("https://devnet-as.magicblock.app/")
      // — use it as-is; only prefix when it is a bare hostname.
      const ep = st.fqdn.startsWith("http") ? st.fqdn : "https://" + st.fqdn + "/";
      erConn = new Connection(ep, "confirmed");
      $("er-endpoint").textContent = st.fqdn;
      return st;
    }
  } catch (e) { console.warn("router lookup failed, using default ER:", e); }
  erConn = new Connection(DEFAULT_ER, "confirmed");
  $("er-endpoint").textContent = "devnet-as (default)";
  return null;
}

// Test/debug handle (no secrets; lets the headless smoke test exercise the
// client without a wallet). Harmless in production.
window.__arenaRush = {
  computePdas, decodeArena,
  ixInitialize, ixInitRewardMint, ixDelegate, ixStartRound, ixTap,
  ixEndRound, ixForceExpire, ixCommit, ixClaim,
  setPdas: (p) => { pdas = p; },
  getPdas: () => pdas,
};

// ---------- wallet ----------
function setWallet(w, isDemo) {
  wallet = w; demoMode = isDemo;
  $("wallet-status").textContent = "Connected: " + short(w.publicKey)
    + (isDemo ? " (demo)" : " (Phantom)");
  $("demo-banner").classList.toggle("hidden", !isDemo);
  $("btn-enter").disabled = false;
}
function keypairWallet(kp) {
  return {
    publicKey: kp.publicKey,
    signTransaction: async (tx) => { tx.partialSign(kp); return tx; },
  };
}
function phantomWallet() {
  return {
    publicKey: window.solana.publicKey,
    signTransaction: (tx) => window.solana.signTransaction(tx),
  };
}
async function onConnectPhantom() {
  if (!window.solana || !window.solana.isPhantom) {
    alert("Phantom not detected. Install Phantom or use the demo wallet.");
    return;
  }
  await window.solana.connect();
  setWallet(phantomWallet(), false);
  await afterConnect();
}
async function onDemoLocalhost() {
  const host = window.location.hostname;
  if (host !== "localhost" && host !== "127.0.0.1") {
    alert("Demo-wallet auto-load works on localhost only. Use the keypair file picker instead.");
    return;
  }
  const res = await fetch("./demo-keypair.json");
  if (!res.ok) {
    alert("demo-keypair.json not found next to this page (localhost recording setup).");
    return;
  }
  const secret = new Uint8Array(await res.json());
  setWallet(keypairWallet(Keypair.fromSecretKey(secret)), true);
  await afterConnect();
}
async function onKeypairFile(file) {
  try {
    const secret = new Uint8Array(JSON.parse(await file.text()));
    if (secret.length !== 64) throw new Error("expected a 64-byte keypair JSON array");
    setWallet(keypairWallet(Keypair.fromSecretKey(secret)), true);
    await afterConnect();
  } catch (e) { alert("Could not load keypair file: " + e.message); }
}
async function afterConnect() {
  pdas = await computePdas(wallet.publicKey);
  await refreshErEndpoint();
  try { await refreshLobby(); }
  catch (e) { logLobby("Lobby refresh failed (RPC unreachable?): " + esc(e.message || e), "err"); }
}
// H-2: lobby state dispatch — after a reload the lobby reads the LIVE arena
// state (ER copy when delegated — the base copy is a stale snapshot while
// delegated) and routes the action button to the right recovery path instead
// of always offering a fresh "Enter".
async function refreshLobby() {
  try {
    const bal = await baseConn.getBalance(wallet.publicKey);
    $("sol-balance").textContent = (bal / 1e9).toFixed(4) + " SOL";
  } catch (e) { $("sol-balance").textContent = "— (RPC error)"; }
  const st = await getDelegationStatus(pdas.arena);
  const delegated = !!(st && (st.isDelegated ?? st.delegated));
  $("delegated-state").textContent = delegated ? "yes — " + st.fqdn : "no";
  arenaState = delegated ? await fetchArena(erConn) : await fetchArena(baseConn);
  if (!arenaState && delegated) arenaState = await fetchArena(baseConn); // ER read fallback
  const btn = $("btn-enter");
  if (!arenaState) {
    $("arena-state").textContent = "not created yet";
    $("best-score").textContent = "—";
    lobbyAction = "enter";
    btn.textContent = "Enter the Arena";
  } else {
    // L-6: guard the STATE_NAMES fallthrough (unknown state byte → UNKNOWN).
    $("arena-state").textContent = (STATE_NAMES[arenaState.state] ?? "UNKNOWN")
      + " (round " + arenaState.roundNonce + ")";
    $("best-score").textContent = arenaState.bestScore;
    switch (arenaState.state) {
      case 0: case 4: // IDLE / CLAIMED — fresh round
        lobbyAction = "enter";
        btn.textContent = "Enter the Arena";
        break;
      case 1: { // PLAYING — resume with the remaining time
        lobbyAction = "resume";
        const left = Math.max(0, ROUND_SECONDS - (Date.now() / 1000 - Number(arenaState.roundStartTs)));
        btn.textContent = "Resume round (" + left.toFixed(0) + "s left)";
        break;
      }
      case 2: // ROLLING — VRF pending, check status / force-expire if expired
        lobbyAction = "check-vrf";
        btn.textContent = "Check VRF status";
        break;
      case 3: // RESOLVED — claim, never re-delegate (H-1)
        if (delegated) {
          lobbyAction = "commit-claim";
          btn.textContent = "Commit & Claim";
        } else {
          lobbyAction = "claim";
          const reward = Math.min(arenaState.score * arenaState.multiplier, MAX_REWARD);
          btn.textContent = "Claim " + reward + " RUSH";
        }
        break;
      default:
        lobbyAction = "enter";
        btn.textContent = "Enter the Arena";
    }
  }
  const ata = await baseConn.getAccountInfo(pdas.ata);
  if (ata) {
    const dv = new DataView(ata.data.buffer, ata.data.byteOffset);
    $("rush-balance").textContent = dv.getBigUint64(64, true).toString() + " RUSH";
  } else $("rush-balance").textContent = "0 RUSH";
}

// ---------- lobby recovery paths (H-2) ----------
// RESOLVED + not delegated: claim straight from the lobby.
async function claimFromLobby() {
  const player = wallet.publicKey;
  const baseState = await fetchArena(baseConn);
  if (!baseState || baseState.state !== 3) {
    logLobby("Nothing to claim — the round is not RESOLVED on base.", "err");
    await refreshLobby();
    return;
  }
  logLobby("Claiming your RUSH reward on base…");
  const claimSig = await sendAndConfirm(baseConn, ixClaim(player), "claim_reward");
  logLobby(`Claimed! <a href="${explorerTx(claimSig)}" target="_blank" rel="noopener">view tx</a>`, "ok");
  addProof("claim_reward (base)", claimSig);
  const reward = Math.min(baseState.score * baseState.multiplier, MAX_REWARD);
  $("res-score").textContent = baseState.score;
  $("res-mult").textContent = "×" + baseState.multiplier;
  $("res-rush").textContent = reward + " RUSH";
  $("res-best").textContent = baseState.bestScore;
  showScreen("screen-results");
}
// RESOLVED + delegated: settle via the chest screen, then claim.
async function commitClaimFromLobby() {
  $("chest-title").textContent = "Settling your resolved round…";
  $("chest-sub").textContent = "Committing the resolved state back to base, then claiming your RUSH.";
  $("chest-result").classList.add("hidden");
  showScreen("screen-chest");
  await commitAndClaim();
}
// PLAYING: re-enter the round with the remaining time and the chain score.
async function resumeRound() {
  const st = await fetchArena(erConn);
  if (!st || st.state !== 1) {
    logLobby("Round is no longer PLAYING — refreshing…", "err");
    await refreshLobby();
    return;
  }
  const remaining = ROUND_SECONDS - (Date.now() / 1000 - Number(st.roundStartTs));
  arenaState = st;
  $("round-nonce").textContent = st.roundNonce.toString();
  if (remaining <= 0) {
    logLobby("Round time is up — ending the round…");
    endRound();
    return;
  }
  logLobby("Resuming round — " + remaining.toFixed(0) + "s left, score " + st.score + ".");
  beginPlay(remaining, st.score);
}
// ROLLING: watch the VRF resolution from the chest screen.
function checkVrfFromLobby() {
  $("chest-title").textContent = "🎲 Checking your VRF roll…";
  $("chest-sub").textContent = "Your round ended and the VRF oracle is resolving the loot multiplier. This usually takes a few seconds.";
  $("chest-result").classList.add("hidden");
  $("btn-force-expire").classList.add("hidden");
  showScreen("screen-chest");
  pollForResolved();
}

// ---------- game flow ----------
// H-2: the lobby button routes by live arena state (see refreshLobby).
async function onEnterArena() {
  $("btn-enter").disabled = true;
  try {
    switch (lobbyAction) {
      case "claim": await claimFromLobby(); break;
      case "commit-claim": await commitClaimFromLobby(); break;
      case "resume": await resumeRound(); break;
      case "check-vrf": checkVrfFromLobby(); break;
      default: await enterArenaFresh(); break;
    }
  } catch (e) {
    logLobby("Error: " + esc(e.message || e), "err");
    $("btn-enter").disabled = false;
  }
}
async function enterArenaFresh() {
  const player = wallet.publicKey;
  try {
    // 1. Reward mint (one-time, permissionless — safe to call if missing).
    if (!await baseConn.getAccountInfo(pdas.mint)) {
      logLobby("Creating RUSH reward mint (one-time)…");
      const sig = await sendAndConfirm(baseConn, ixInitRewardMint(player), "init_reward_mint");
      logLobby(`Mint created. <a href="${explorerTx(sig)}" target="_blank" rel="noopener">view tx</a>`, "ok");
    }
    // 2. Arena PDA.
    if (!await baseConn.getAccountInfo(pdas.arena)) {
      logLobby("Creating your arena account…");
      const sig = await sendAndConfirm(baseConn, ixInitialize(player), "initialize");
      logLobby(`Arena created. <a href="${explorerTx(sig)}" target="_blank" rel="noopener">view tx</a>`, "ok");
      addProof("initialize (base)", sig);
    }
    // 3. Delegate to the ER (skip if already delegated).
    // Live router field is `isDelegated` (not `delegated`).
    const st = await getDelegationStatus(pdas.arena);
    if (!st || !(st.isDelegated ?? st.delegated)) {
      // H-1: a RESOLVED round routes to claim, NEVER to re-delegate (the
      // program guard would reject it anyway — this keeps the UX honest).
      const fresh = await fetchArena(baseConn);
      if (fresh && fresh.state === 3) {
        logLobby("Round already resolved — routing to claim instead of re-delegating.");
        await claimFromLobby();
        return;
      }
      logLobby("Delegating arena to the Ephemeral Rollup…");
      const sig = await sendAndConfirm(baseConn, ixDelegate(player), "delegate");
      logLobby(`Delegated. <a href="${explorerTx(sig)}" target="_blank" rel="noopener">view tx</a>`, "ok");
      addProof("delegate (base)", sig);
      await refreshErEndpoint();
    } else {
      logLobby("Arena already delegated to " + st.fqdn + ".");
    }
    // 4. Start the round on the ER.
    logLobby("Starting round on the ER…");
    let sig;
    try {
      sig = await sendAndConfirm(erConn, ixStartRound(player), "start_round");
    } catch (e) {
      // H-2 recovery: the arena may have been undelegated (or never
      // delegated) — re-poll the router once and re-delegate if needed,
      // then retry start_round once before surfacing the error.
      logLobby("start_round failed — re-checking delegation…");
      const st2 = await getDelegationStatus(pdas.arena);
      if (!st2 || !(st2.isDelegated ?? st2.delegated)) {
        logLobby("Arena not delegated — re-delegating once…");
        const dsig = await sendAndConfirm(baseConn, ixDelegate(player), "delegate");
        logLobby(`Re-delegated. <a href="${explorerTx(dsig)}" target="_blank" rel="noopener">view tx</a>`, "ok");
        addProof("delegate (base, retry)", dsig);
        await refreshErEndpoint();
        sig = await sendAndConfirm(erConn, ixStartRound(player), "start_round");
      } else { throw e; }
    }
    logLobby(`Round started. <a href="${explorerTx(sig)}" target="_blank" rel="noopener">view tx</a>`, "ok");
    addProof("start_round (ER)", sig);
    arenaState = await fetchArena(erConn);
    $("round-nonce").textContent = arenaState ? arenaState.roundNonce.toString() : "—";
    beginPlay();
  } catch (e) {
    logLobby("Error: " + esc(e.message || e), "err");
    $("btn-enter").disabled = false;
  }
}
// remaining/startScore support the H-2 resume path (fresh rounds use defaults).
function beginPlay(remaining = ROUND_SECONDS, startScore = 0) {
  roundActive = true; endingRound = false;
  confirmedTaps = startScore; pendingTaps = 0; updateTapCounts();
  $("score").textContent = String(startScore);
  showScreen("screen-arena");
  spawnTarget();
  const t0 = Date.now();
  clearInterval(roundTimer);
  roundTimer = setInterval(() => {
    const left = remaining - (Date.now() - t0) / 1000;
    $("timer").textContent = Math.max(0, left).toFixed(1);
    if (left <= 0) { clearInterval(roundTimer); endRound(); }
  }, 100);
}
function spawnTarget() {
  const t = $("target");
  const area = $("tap-area");
  const x = 8 + Math.random() * 84, y = 10 + Math.random() * 78;
  t.style.left = x + "%"; t.style.top = y + "%";
  t.classList.remove("hidden");
}
function burst(x, y) {
  const fb = $("tap-feedback");
  const b = document.createElement("div");
  b.className = "burst"; b.textContent = "+1";
  b.style.left = x + "px"; b.style.top = y + "px";
  fb.appendChild(b);
  setTimeout(() => b.remove(), 650);
}
function onTap(ev) {
  if (!roundActive || endingRound) return;
  // Optimistic UI: score + target move instantly; chain confirms behind.
  const el = $("score");
  el.textContent = String(Number(el.textContent) + 1);
  const area = $("tap-area"), r = area.getBoundingClientRect();
  burst(ev.clientX - r.left, ev.clientY - r.top);
  let tid;
  do { tid = Math.floor(Math.random() * 65534); } while (tid === lastTargetId);
  lastTargetId = tid;
  spawnTarget();
  sendTapFireAndForget(tid);
}
async function endRound() {
  if (endingRound) return;
  endingRound = true; roundActive = false;
  $("target").classList.add("hidden");
  showScreen("screen-chest");
  $("chest-title").textContent = "🎲 Rolling verifiable randomness…";
  $("chest-sub").textContent = "The VRF oracle is proving your loot multiplier on-chain. This takes a few seconds.";
  $("chest-result").classList.add("hidden");
  $("btn-force-expire").classList.add("hidden");
  try {
    const sig = await sendAndConfirm(erConn, ixEndRound(wallet.publicKey), "end_round");
    logChest(`VRF request sent. <a href="${explorerTx(sig)}" target="_blank" rel="noopener">view tx</a>`, "ok");
    addProof("end_round / VRF request (ER)", sig);
    pollForResolved();
  } catch (e) {
    logChest("end_round failed: " + esc(e.message || e), "err");
    $("chest-title").textContent = "Something went wrong — check the log.";
  }
}
// M-5: the poll now CAPS (~160 s — covers the 130 s VRF timeout + margin),
// reconciles the optimistic tap score from chain state on every tick, and
// ends in a visible manual-resolution state instead of spinning forever.
async function pollForResolved() {
  clearInterval(pollTimer);
  let polls = 0;
  const MAX_POLLS = 200;
  pollTimer = setInterval(async () => {
    try {
      const st = await fetchArena(erConn);
      if (!st) return;
      arenaState = st;
      confirmedTaps = st.score;
      $("score").textContent = String(st.score);
      updateTapCounts();
      if (st.state === 3) {           // RESOLVED — VRF callback landed
        clearInterval(pollTimer);
        revealChest(st);
        return;
      }
      if (st.state === 2) {    // ROLLING
        const age = Date.now() / 1000 - Number(st.vrfRequestTs);
        if (age > VRF_TIMEOUT_S) $("btn-force-expire").classList.remove("hidden");
      }
      if (++polls >= MAX_POLLS) {
        clearInterval(pollTimer);
        $("chest-title").textContent = "Still waiting for the VRF oracle…";
        $("chest-sub").textContent = "The round is still pending. You can resolve it manually as Common ×1 (once 130 s have passed) or reload and check again from the lobby.";
        $("btn-force-expire").classList.remove("hidden");
        logChest("VRF poll gave up after ~160 s — round still pending. Use force-expire (after 130 s) or retry from the lobby.", "err");
      }
    } catch (e) { console.warn("poll:", e); }
  }, 800);
}
function revealChest(st) {
  const [name, cls, emoji] = TIERS[st.multiplier] || ["Common", "common", "⚪"];
  $("chest").textContent = "🎁"; $("chest").classList.add("open");
  $("chest-title").textContent = "Your loot chest is open!";
  $("chest-sub").textContent = "The multiplier was rolled by Solana VRF — verifiable on-chain.";
  $("chest-tier").textContent = `${emoji} ${name} ×${st.multiplier}`;
  $("chest-tier").className = "tier " + cls;
  const reward = Math.min(st.score * st.multiplier, MAX_REWARD);
  $("chest-math").textContent = `Score ${st.score} × ${st.multiplier} = ${reward} RUSH`;
  $("chest-result").classList.remove("hidden");
  logChest(`VRF resolved: ${name} ×${st.multiplier}, score ${st.score} → ${reward} RUSH.`, "ok");
  setTimeout(commitAndClaim, 2500);
}
async function onForceExpire() {
  $("btn-force-expire").disabled = true;
  try {
    const sig = await sendAndConfirm(erConn, ixForceExpire(wallet.publicKey), "force_expire");
    logChest(`Force-expired (Common ×1 fallback). <a href="${explorerTx(sig)}" target="_blank" rel="noopener">view tx</a>`, "ok");
    addProof("force_expire (ER)", sig);
  } catch (e) {
    // M-5: never leave the button dead — re-enable with a clear message.
    // VrfNotExpired is custom program error 0x1778 (6008).
    const msg = e.message || String(e);
    const notExpired = msg.includes("0x1778") || msg.includes("6008")
      || msg.toLowerCase().includes("not expired");
    logChest("force_expire failed: " + esc(msg)
      + (notExpired ? " — the 130 s VRF timeout has not passed yet on-chain. Wait a bit and try again." : ""), "err");
    $("btn-force-expire").disabled = false;
  }
}
async function commitAndClaim() {
  const player = wallet.publicKey;
  try {
    logChest("Committing round state back to base + undelegating…");
    const sig = await sendAndConfirm(erConn, ixCommit(player), "commit_and_undelegate");
    logChest(`Commit scheduled. <a href="${explorerTx(sig)}" target="_blank" rel="noopener">view tx</a>`, "ok");
    addProof("commit_and_undelegate (ER)", sig);
    // Extract the base commit signature from the ER tx logs, then confirm it.
    const tx = await erConn.getTransaction(sig, { commitment: "confirmed", maxSupportedTransactionVersion: 0 });
    const logs = (tx && tx.meta && tx.meta.logMessages) || [];
    const m = logs.join("\n").match(/ScheduledCommitSent signature(?:\[0\])?: ([1-9A-HJ-NP-Za-km-z]{32,})/);
    if (m) {
      logChest(`Base commit tx: <a href="${explorerTx(m[1])}" target="_blank" rel="noopener">${m[1].slice(0, 12)}…</a>`);
      addProof("commit (base)", m[1]);
      // Best-effort only: the ScheduledCommitSent sig is not always visible
      // via getTransaction on public RPCs, yet the resolved state still lands
      // on base (proven in the live run 2026-11-20). Never let a confirm
      // timeout abort the flow — the undelegation poll below is the real gate.
      try { await baseConn.confirmTransaction(m[1], "confirmed"); }
      catch (e) { logChest("Base commit confirm timed out (non-fatal — checking undelegation)…"); }
    }
    // Wait until the base account is undelegated again (owner back to program).
    logChest("Waiting for undelegation on base…");
    for (let i = 0; i < 40; i++) {
      const info = await baseConn.getAccountInfo(pdas.arena);
      if (info && info.owner.equals(PROGRAM_ID)) break;
      await new Promise((r) => setTimeout(r, 1500));
    }
    const baseState = await fetchArena(baseConn);
    if (!baseState || baseState.state !== 3) {
      logChest("Base state not RESOLVED after commit — check explorer. state="
        + (baseState ? (STATE_NAMES[baseState.state] ?? "UNKNOWN") : "missing"), "err");
      return;
    }
    logChest("Claiming RUSH reward on base…");
    const claimSig = await sendAndConfirm(baseConn, ixClaim(player), "claim_reward");
    logChest(`Claimed! <a href="${explorerTx(claimSig)}" target="_blank" rel="noopener">view tx</a>`, "ok");
    addProof("claim_reward (base)", claimSig);
    const reward = Math.min(baseState.score * baseState.multiplier, MAX_REWARD);
    $("res-score").textContent = baseState.score;
    $("res-mult").textContent = "×" + baseState.multiplier;
    $("res-rush").textContent = reward + " RUSH";
    $("res-best").textContent = baseState.bestScore;
    showScreen("screen-results");
  } catch (e) {
    logChest("commit/claim failed: " + esc(e.message || e), "err");
  }
}

// ---------- wiring ----------
$("btn-connect").addEventListener("click", onConnectPhantom);
$("btn-demo").addEventListener("click", onDemoLocalhost);
$("file-keypair").addEventListener("change", (e) => {
  if (e.target.files[0]) onKeypairFile(e.target.files[0]);
});
$("btn-enter").addEventListener("click", onEnterArena);
$("target").addEventListener("click", onTap);
$("tap-area").addEventListener("click", (e) => { if (e.target.id === "tap-area") burst(e.offsetX, e.offsetY); });
$("btn-force-expire").addEventListener("click", onForceExpire);
$("btn-again").addEventListener("click", async () => {
  $("proof-links").innerHTML = "";
  showScreen("screen-lobby"); await refreshLobby();
  $("btn-enter").disabled = false; onEnterArena();
});
$("btn-lobby").addEventListener("click", async () => {
  $("proof-links").innerHTML = "";
  showScreen("screen-lobby"); await refreshLobby();
  $("btn-enter").disabled = false;
});
// Auto-reconnect Phantom if it was already authorized.
if (window.solana && window.solana.isPhantom) {
  window.solana.connect({ onlyIfTrusted: true })
    .then(() => { setWallet(phantomWallet(), false); afterConnect(); })
    .catch(() => {});
}
