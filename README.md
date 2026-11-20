# ⚡ ARENA RUSH

**A 30-second reflex game that base-layer Solana physically cannot run — built on MagicBlock Ephemeral Rollups + Solana VRF.**

> **Solana Blitz v10 entry** · Theme: Entertainment Finance · Devnet only
> Submissions close ~Fri Nov 27, 2026, 21:00 WIB.

Tap fast-moving targets for 30 seconds. Every tap is a **gasless, 10 ms-block transaction on an Ephemeral Rollup** — instant feedback that 400 ms base-layer blocks could never deliver. When the round ends, a **Solana VRF loot chest** rolls your multiplier (Common ×1 → Legendary ×5), and your winnings settle on base as **RUSH**, a real SPL token you truly own.

**The rollup is not a cost optimisation here — it is the product.** Speed you can feel, randomness you can verify, winnings you truly own.

---

## ⚠️ Deployment status

| Item | State |
|---|---|
| Program build | ✅ Built — `anchor-1.0.2 build` green, `.so` 419,488 bytes (audit-fix build; v1 was 394,680) |
| MagicSVM tests | ✅ 13/13 passing (see [Tests](#tests)) |
| Frontend | ✅ Built — static site in `docs/`, smoke-tested headless |
| **Devnet deploy** | ✅ **LIVE on devnet since 2026-11-20 11:56 WIB** — [deploy tx](https://explorer.solana.com/tx/m9oodwv3NgknwpNjZkHGVem1nA1epL31ZruACKnT7vsR19U6GNnnUTq12jj9y3cTWwa8SZvt8UGL86VANeHJUb2?cluster=devnet) (slot 524237385) · **Upgraded in place 2026-11-20 (audit fixes, same program ID)** — see [Live proof](#live-proof) |
| **Live proof run** | ✅ **Full loop verified live 2026-11-20 11:57–11:59 WIB** — delegate → 6 taps → VRF Epic ×3 → commit → claim 18 RUSH · **Re-verified after the audit-fix upgrade** (see [Live proof](#live-proof)) |

**Program ID (devnet, LIVE):**
`HDAvFeo4tpoGzmZqvAhRzmYVdF1qi4CCJxQNZjhQj9D2`

Explorer:
https://explorer.solana.com/address/HDAvFeo4tpoGzmZqvAhRzmYVdF1qi4CCJxQNZjhQj9D2?cluster=devnet

---

## How it works

```
┌─────────────────────────────────────────────────────────────────────────┐
│  BASE LAYER (Solana devnet)                                             │
│                                                                         │
│  1. initialize ──► create Arena PDA (94 bytes: player, score, state,    │
│                     multiplier, nonces, timestamps, best score)         │
│  2. delegate ────► Delegation Program locks the Arena PDA and clones    │
│                     it into the devnet-as Ephemeral Rollup              │
│                     (~0.003 SOL session deposit)                        │
│                                                                         │
│  6. commit_and_undelegate returns state to base (see below)             │
│  7. claim_reward ─► mint min(score × multiplier, 1000) RUSH (SPL,       │
│                     decimals 0) into the player's ATA on base           │
└──────────────┬──────────────────────────────────────────────────────────┘
               │ delegate (base tx)
               ▼
┌─────────────────────────────────────────────────────────────────────────┐
│  EPHEMERAL ROLLUP — devnet-as (10 ms blocks, gasless txs)               │
│                                                                         │
│  3. start_round ─► reset score/state, start the 30 s clock              │
│  4. tap ─────────► each tap = one ER tx; score increments live          │
│                     (1 tap/slot cap, target-id replay check, 35 s      │
│                     round window incl. 5 s grace)                       │
│  5. end_round ───► close round, request randomness from the delegated   │
│                     VRF queue — exactly ONE VRF request per round,      │
│                     fee-exempt inside the ER                            │
│       │                                                                 │
│       ▼                                                                 │
│  ┌─────────────────────────────────────────────────────────────────┐    │
│  │  SOLANA VRF (oracle proof, RFC 9381)                            │    │
│  │  vrf_callback ──► resolves the loot multiplier from verified    │    │
│  │  randomness: Common ×1 (55%) · Rare ×2 (30%) · Epic ×3 (11%)   │    │
│  │  · Legendary ×5 (4%). Nonce-bound, idempotent; late callbacks   │    │
│  │  for superseded rounds are rejected.                            │    │
│  └─────────────────────────────────────────────────────────────────┘    │
│                                                                         │
│  6. commit_and_undelegate (ER tx, MagicIntentBundleBuilder)             │
│     — 1 commit per session, far under the 10-commit cap.                │
│     State guard: commit requires RESOLVED (never mid-roll).             │
│     Stuck-round escape: force_expire (permissionless, after 130 s)     │
│     resolves a missing VRF callback as Common ×1 so the player can      │
│     always claim.                                                       │
└─────────────────────────────────────────────────────────────────────────┘
```

### The two eligibility primitives

Arena Rush uses **both** of MagicBlock's core primitives in a single game loop:

| Primitive | Program | How Arena Rush uses it |
|---|---|---|
| **Ephemeral Rollup** | Delegation Program `DELeGGvXpWV2fqJUhqcF5ZSYMS4JTLjteaAMARRSaeSh` | The Arena PDA is delegated to `devnet-as` (SGP, 10 ms blocks, baseFee 0) via the router. All gameplay (`start_round`, `tap`, `end_round`, `vrf_callback`, `commit_and_undelegate`) runs on the rollup — gasless, instant. Without the ER, a real-time reflex game is physically impossible: base-layer ~400 ms blocks are slower than a human tap. |
| **Solana VRF** | VRF Program `Vrf1RNUjXmQGjmQrQLvJHs9SNkvDJEsRVFPkfSQUwGz` | `end_round` requests randomness from the delegated queue `DEFAULT_EPHEMERAL_QUEUE` (`5hBR571xnXppuCPveTrctfTU7tJLSN94nq7kv7FRK5Tc`, fee-exempt inside the ER). The oracle's RFC 9381 proof is verified on-chain before the `#[vrf_callback]` resolves the loot multiplier. One request per round; queue pinned to `DEFAULT_EPHEMERAL_QUEUE` only. |

Program stack: **Anchor 1.0.2** · **Solana 3.1.9** · **ephemeral-rollups-sdk 0.18.0** (`anchor` + `vrf` features) · Rust 1.89.0.
SDK 0.18.0 is pinned deliberately — 0.20.0 is the validator-2.0 line and its `#[vrf]` macro does not compile.

---

## Program instructions (11)

| Instruction | Layer | Purpose |
|---|---|---|
| `initialize` | base | Create the Arena PDA for a player |
| `delegate` | base | Delegate the Arena PDA to the ER (`#[delegate]`) |
| `start_round` | ER | Reset score/state, start the 30 s round clock |
| `tap` | ER | Validate target id, increment score (1 tap/slot, 35 s window) |
| `end_round` | ER | Close round, request VRF randomness (`#[vrf]`) |
| `vrf_callback` | ER | `#[vrf_callback]` — resolve loot multiplier from verified randomness |
| `force_expire` | ER | Permissionless stuck-round recovery after 130 s → Common ×1 |
| `commit_and_undelegate` | ER | Commit state + undelegate (`#[commit]`, requires RESOLVED) |
| `process_undelegation` | — | Injected by `#[ephemeral]` (callback CPI) |
| `init_reward_mint` | base | One-time setup of the RUSH reward mint (program-owned authority PDA) |
| `claim_reward` | base | Mint `min(score × multiplier, 1000)` RUSH to the player's ATA |

Arena PDA: **94 bytes** — far under the 10,240-byte commit limit. Commits per session: **1** — far under the 10-commit cap.

---

## Run it

### Play the game (devnet)

The program is **live on devnet**.

- **Live demo:** https://arefki.github.io/arena-rush/
- **Demo video:** https://www.youtube.com/watch?v=lXy6oL0YaA0 (2:51, gameplay captured live on devnet)

Requirements: a Solana wallet (Phantom) on **devnet** with a little devnet SOL (faucet: https://faucet.solana.com). One round costs ~0.003 SOL session deposit + base tx fees; taps and the VRF request inside the ER are free.

### Run the frontend locally

The frontend is a static site — no build step, no backend.

```bash
cd docs
python3 -m http.server 8000   # or any static server
# open http://localhost:8000
```

On `localhost` a **"Demo wallet (localhost)"** button appears: place a devnet keypair JSON at `docs/demo-keypair.json` (gitignored, never committed, never served by Pages) or pick any devnet keypair file. A big **DEVNET ONLY** banner shows while demo-wallet mode is active. Phantom is the normal path.

The site is GitHub Pages-ready: `docs/` with relative paths only, vendored `@solana/web3.js` 1.98.0 (UMD, `docs/vendor/`). The client hand-encodes instructions from the IDL (8-byte discriminator + borsh args) — no Anchor TS client, no TS SDK dependency.

**4 screens:** Lobby (status grid + state-aware action button — Enter / Claim / Commit & Claim / Resume / Check VRF) → Arena (DOM targets, 30 s countdown, optimistic taps) → Loot chest (VRF reveal + `force_expire` fallback button after 130 s) → Results (score, multiplier, RUSH won, proof tx links).

### Build the program

```bash
# Official stack: Anchor 1.0.2 + Solana/Agave 3.1.9, Rust 1.89.0
anchor build
# output: target/deploy/arena_rush.so, target/idl/arena_rush.json
```

Exact pins are required in `programs/arena-rush/Cargo.toml`: `anchor-lang = "=1.0.2"`, `anchor-spl = "=1.0.2"`, `ephemeral-rollups-sdk = "=0.18.0"`. Bare `"1.0.2"` resolves to anchor-lang 1.2.2, which pulls solana-program 4.0 and breaks the SDK `#[vrf]` macro. Never run a bare `cargo update` — it re-maximizes the anchor derive crates to 1.2.2.

### Deploy (devnet)

```bash
anchor deploy --provider.cluster devnet
# then one-time: init_reward_mint (creates the RUSH mint)
```

Deploy needs ~2 SOL program rent (419,488-byte program; buffer rent ~2.13 SOL, refunded after the upgrade). ✅ **Deployed 2026-11-20 11:56 WIB** — [deploy tx](https://explorer.solana.com/tx/m9oodwv3NgknwpNjZkHGVem1nA1epL31ZruACKnT7vsR19U6GNnnUTq12jj9y3cTWwa8SZvt8UGL86VANeHJUb2?cluster=devnet). Note: the first `anchor deploy` attempt timed out mid-upload (slow RPC); the deploy was resumed with `solana program deploy --buffer <buffer-address>` and finalized in ~3 s. The IDL is also initialized on-chain.

✅ **Upgraded in place 2026-11-20 ~15:14 WIB (audit fixes, same program ID)** — [upgrade tx](https://explorer.solana.com/tx/3DthULqP8PRfYGKTRhMEXTCKTvuPamEGhGSB1ynrkKLFX5jJqGpMqiLCRAn9vzgjYHrxGNkqb8vbjRfuuuwMDaa8?cluster=devnet) (slot 524288575). Verified on-chain: program data 419,488 bytes, authority unchanged (`BZVys…wbF`), live arena state intact (CLAIMED, round 1, best 6). Upgrade cost 0.128 SOL (program-data extension rent + fees; buffer rent refunded). The upgrade is state-compatible — the Arena layout is unchanged (94 bytes) and the `player_stats` ledger is created lazily on the next claim.

---

## Tests

MagicSVM 0.3.0 (in-process base + ephemeral ledgers, incl. VRF callbacks) — no local validator needed.

```bash
cargo +stable test -p arena-rush-magicsvm-tests   # ~35 s
```

**Result: 13/13 passing** (verified 2026-11-20, audit-fix build):

| Test | Covers |
|---|---|
| `full_lifecycle` | init → delegate → 5 taps → VRF resolve → commit → claim; RUSH minted = score × multiplier; anti-replay ledger recorded; second claim rejected (exact code) |
| `guards` | tap / end_round / commit before their states fail (exact codes); target replay rejected; force_expire while PLAYING rejected; claim while delegated → 3007 owner check |
| `force_expire_recovers_stuck_round` | 130 s timeout → Common ×1; late VRF callback afterwards rejected (exact code); commit + claim still work |
| `max_reward_cap` | 201 taps × Legendary ×5 = 1005 capped to 1000 RUSH |
| `round_time_window` | tap 40 s after round start → `RoundTimeExpired` (6007) |
| `one_tap_per_slot` | two taps, same slot, different targets → `TapRateLimited` (6005) |
| `stale_callback_two_requests` | lost round-1 callback delivered during round 2 → `StaleCallback` (6004); round-2 callback still resolves |
| `round_not_startable` | `start_round` from PLAYING, ROLLING and RESOLVED → `RoundNotStartable` (6000) |
| `cross_player_isolation` | foreign signer tap / end_round / claim → Anchor `ConstraintSeeds` (2006) |
| `delegate_guard_h1` | delegate from RESOLVED → `CannotDelegateUnclaimed` (6009); delegate after claim works, round 2 plays |
| `force_expire_permissionless_m1` | a stranger expires another player's stuck ROLLING arena |
| `claim_replay_blocked_m4` | forged snapshot revert to RESOLVED → replay claim rejected by the stats ledger (`RoundAlreadyClaimed`, 6010) |
| `claim_not_resolved_undelegated` | claim on an IDLE undelegated arena → `NotResolved` (6003) |

All negative assertions pin the **exact error code** (`Custom(<code>)` / `custom program error: 0x<code>`) — the pre-audit tests used `|| err.contains("Custom")`, which every Anchor custom error satisfies, so the guard tests were vacuous (audit H-3). Pinning the codes caught a real discrepancy: the old "claim before resolve → NotResolved" assertion was actually failing with `AccountOwnedByWrongProgram` (3007) all along, because the arena was delegated at that point.

MagicSVM caught a real bug the build would otherwise have shipped: the `#[vrf_callback]` macro's runtime PDA bump search consumed the entire ~200k CU budget before the handler ran. Fixed by declaring the scoped VRF identity explicitly (`.so` shrank 425,664 → 395,352 bytes at that build; the v1 release `.so` was 394,680 bytes, the audit-fix build is 419,488). This would have failed on real devnet too — the VRF oracle's fulfillment tx has the same budget.

**MagicSVM quirk (documented, not a program bug):** after `commit_and_undelegate`, MagicSVM leaves the delegation buffer/record/metadata accounts on base, and its `apply_base_account_state` re-delegates the arena on the next base tx that writes it. The real DLP closes those accounts when undelegation finalizes. Tests call `finalize_undelegation` (clears the three accounts via `set_account`) after every commit to mirror the chain.

Frontend smoke test (Playwright, headless): page renders, web3 bundle loads, arena PDA derives, `decodeArena` parses a synthetic 94-byte buffer, tap instruction encodes correctly. Demo-wallet mode verified headless (connects, DEVNET ONLY banner, zero page errors).

---

## Live proof

> ✅ **Verified live on devnet 2026-11-20 11:57–11:59 WIB** (round 1, player `BZVys2yLbDQ9T4WbBjLXTi1DVeQqTDi6Jysjdk8cXwbF`, ER `devnet-as`). Full loop: delegate → start → **6 taps, all confirmed** → VRF request → **real VRF callback (Epic ×3)** → commit → undelegate → **claim 18 RUSH** (score 6 × 3). Every link below was verified via RPC.

| Step | Transaction (devnet explorer) |
|---|---|
| Program deploy | [m9oodwv3…HJUb2](https://explorer.solana.com/tx/m9oodwv3NgknwpNjZkHGVem1nA1epL31ZruACKnT7vsR19U6GNnnUTq12jj9y3cTWwa8SZvt8UGL86VANeHJUb2?cluster=devnet) (slot 524237385) |
| `init_reward_mint` | [LGWguoEE…cNx1](https://explorer.solana.com/tx/LGWguoEEauvVohSS4XigHFbD9Mxep9TZaAqFEXiBwyEFa5uKba7BrU5Hi4EStDBG4NGfcH5W6gDaPPEVP9jcNx1?cluster=devnet) |
| `initialize` (arena PDA) | [vVNSCbD9…vvL5x](https://explorer.solana.com/tx/vVNSCbD93aDXZEfdgtA4U5pWqs26oSYoXRXMZ1rjsirYCfPzvvUyePXnZaWEBS64eraW3b4DgXZuqk9LLGvvL5x?cluster=devnet) |
| `delegate` | [xvTwRkEV…AYyN](https://explorer.solana.com/tx/xvTwRkEVTAWrbzXu7B6m1eSYQ7qEHzZpqvwFa645UWXQAtKqvVhNKuCR2JCFAzGkFwXuwzgv9swUwgt6rp7AYyN?cluster=devnet) |
| `start_round` (ER) | [3DVWH2Kb…DRPJ](https://explorer.solana.com/tx/3DVWH2KbvRN4qs99DE7nAjKR4QPFQV3u8ugvoy47dnSfUbyPz9NEwKmZLfUL6gaNaojQnGsXsQSD5ea312VqDRPJ?cluster=devnet) |
| `tap` ×6 (ER, all finalized) | [#1 5TC3jf49…Gg3J](https://explorer.solana.com/tx/5TC3jf49yTU5hnYUtRTvZVqFHWYWZGZidSd9UqA6v3cvavz4ZR2jgzzF3jM9hAHGJQJPZrqtr9MET5mKEgveGg3J?cluster=devnet) · [#2 4Q1M6Rcu…vFza](https://explorer.solana.com/tx/4Q1M6Rcu4jQM3MZW5YFChGHvFpzX18SWsuo1T94TgqJGrA17Dbg4CMgDwJh23cdVtkcXB2a9eNcsJJomxx3uvFza?cluster=devnet) · [#3 qX4AMW9E…cwni](https://explorer.solana.com/tx/qX4AMW9EFKonCp8DH93YPeA1Ro8DFittAtF1PMQLem99EGhQDVcUUqwEyTJsPUtc1HUShjW35nLHpfqfs6qcwni?cluster=devnet) · [#4 21Z8o8Sj…spLF](https://explorer.solana.com/tx/21Z8o8Sjg6diKENn3x5fUJpQj567pZnUBpCB8HiL4tCEhyJc9JmbySxdCtT7tu2AfaRpf1DWndYCjRZnrZKyspLF?cluster=devnet) · [#5 2csc3KsU…mg4Y](https://explorer.solana.com/tx/2csc3KsUpnMRb6mkkgDNgtXAJAi6BTDGAXaEYkcbehhfGyMhiX7MchLcpTmV64oRqgurHUXxoRPbhHFyjPL9mg4Y?cluster=devnet) · [#6 566579zM…CftF](https://explorer.solana.com/tx/566579zM5cJHXgRmuSKuToofJdMMWHhRCKLPALRZfaLJNYhXcKMCXQPF1xDeisXfEJsTgkeUE3sYmzSUFTV7CftF?cluster=devnet) |
| `end_round` (VRF request, ER) | [2RnV8eZA…egYYP](https://explorer.solana.com/tx/2RnV8eZAR3ESv3QcYt7rKyEijxLoXBUonBNDwuFYZ85C6LqUyDDcL1Fi5To3B5U6X98kvFzZw3Zu6a3TCqAegYYP?cluster=devnet) |
| `vrf_callback` (loot reveal, ER) | [BqiXPoNq…WEk7p](https://explorer.solana.com/tx/BqiXPoNqwvSNuCGBZ81F82DAUXvmZKorNCNwShrAQeckpStmCzWiiCQZzgbxZkgrquEZcFBUieUWrxDfdaWEk7p?cluster=devnet) — VRF program invoked `VrfCallback`, 2,676 CU, fee 0 |
| `commit_and_undelegate` (ER) | [45cYrf4K…HbQz](https://explorer.solana.com/tx/45cYrf4KXX4KUiBHCC8tFtuKAqPbYBfQnnkjVaKaiMudaPFFc6YYJXvX13KxvETKzHFt7U8YaxmjdQGEjpD2HbQz?cluster=devnet) |
| `claim_reward` (18 RUSH minted) | [28xM9FBi…6xHc](https://explorer.solana.com/tx/28xM9FBitfqYsowbbELP6ug2Q9GQ3GnXTBiZn1ASG54TScwUwhQ92VkrXav3wc3mkotB4jb1XDddbgBXF2Ys6xHc?cluster=devnet) |

**Measured on the live run (honest numbers):**
- **VRF callback landed 3 ER slots (~30 ms) after the `end_round` request** (slots 1117504828 → 1117504831). The client observed the resolution in 2.2 s (polling cadence + proxy RTT, not the oracle).
- **Taps:** client-measured send→finalized ~518–523 ms each through this VM's egress proxy (~1 s RTT per RPC call). The ER itself runs 10 ms blocks — tap txs landed ~78 slots (~780 ms) apart, matching the client's pacing, not any ER bottleneck. A real browser (no proxy) will see taps confirm in well under 100 ms.
- **VRF callback cost:** 2,676 CU (the explicit-identity fix; the pre-fix version blew the ~200k CU budget in MagicSVM).
- **Commit:** the base commit signature from the ER logs (`ScheduledCommitSent`) is not visible via `getTransaction` on the public devnet RPC, but the resolved state (score 6, ×3, RESOLVED) provably landed on base and the arena was undelegated (owner back to the program) before the claim — verified by direct account reads.
- RUSH mint: `6wVRctFVeHTFAm7PEASbnUAkXLPjociFTkq9qtXmvYmy` · Arena PDA: `Fq5i8LGZVbfBh6dAoLSzcjFmDvHDrR4zFAX7fYkjpbf7`

> ✅ **Re-verified live after the audit-fix upgrade — round 2, 2026-11-20 15:16–15:17 WIB** (same player, ER `devnet-as`). Full loop on the upgraded program: delegate (CLAIMED arena re-delegates cleanly under the H-1 guard) → start round 2 → **6 taps, all confirmed** → VRF request (high-priority, L-7) → **real VRF callback 2 ER slots later (Rare ×2)** → commit → undelegate → **claim 12 RUSH** (score 6 × 2). RUSH balance 18 → 30. The `player_stats` ledger was created live (`3cJ4zYpFd6YshwZGZqwr2Em4HmKZ9zfnFPbNJNU3N3z4`: `last_claimed_nonce` 2, `total_minted` 12 — round 1's 18 RUSH predates the ledger).

| Step (round 2) | Transaction (devnet explorer) |
|---|---|
| Program upgrade | [3DthULqP…MDaa8](https://explorer.solana.com/tx/3DthULqP8PRfYGKTRhMEXTCKTvuPamEGhGSB1ynrkKLFX5jJqGpMqiLCRAn9vzgjYHrxGNkqb8vbjRfuuuwMDaa8?cluster=devnet) (slot 524288575) |
| `delegate` | [4dA7g4Di…iuVu](https://explorer.solana.com/tx/4dA7g4Dijot3ecPGzFgJ9WJK4KaYmHWAABvw2RZvxg6kAJ9rkiS7NAq5EwXEYxmf7NKpDiVSddhjy87CzR47iuVu?cluster=devnet) |
| `start_round` (ER) | [3bbW6TMt…VxeA](https://explorer.solana.com/tx/3bbW6TMtCzcuYVfSSeFupkrsYRSSa9nmqQ8JjUFBUgWcydjFYi7X74yLe26yAsPUxGh2eJ4ivwLpEv2aAPBMVxeA?cluster=devnet) |
| `tap` ×6 (ER, all finalized) | [#1 35WZisxw…DHLv](https://explorer.solana.com/tx/35WZisxwR2Jz54ruMtr7ZJmWnLCDgUt2mGorkfRdesipvjHvXtyxLJ4GiZ9a5PxxhkyTsGQTmaN7tHyduyiDDHLv?cluster=devnet) · [#2 26Tq5i4f…e4Mc](https://explorer.solana.com/tx/26Tq5i4fu2GCSicmNMt9vPT48tnxK5n6PhjRunfT4BvQGGRXvy3H9H6a9NGsHjG9SM6MDCVfQ8NWAgQXGih8e4Mc?cluster=devnet) · [#3 5coWNABH…73P1y](https://explorer.solana.com/tx/5coWNABHek2k8D5BEp53U6C4LuhPzFWqx5RCncygBx6U9hJvHBbCGgqmQv4i2c3SzpMFBjx3fgtbWWPfGN373P1y?cluster=devnet) · [#4 Q5Ptt3fv…kRJe](https://explorer.solana.com/tx/Q5Ptt3fv97po2yeEEpNBcFYCVSfgoJZVvddZwvTiPVd7emqikaMsLSyNDFuRrjzEfJZCdm9bp6F8jN1Eww4kRJe?cluster=devnet) · [#5 2Ad4rjdB…9yigq](https://explorer.solana.com/tx/2Ad4rjdBJ7px5dyuM3J7KSy26nqdRZyMSPWkkYogubHGHGUc6AUtsvv7jmHnGTgKzd6fJXFkiZkdXCsKsMZ9yigq?cluster=devnet) · [#6 4UguVroA…CUFK](https://explorer.solana.com/tx/4UguVroA7qLYRksPDKBe31UUNTDn4ESzFxJG2ed2WamjuSNCnp3CUDZUGfbKgoiQ14MRdg5drfoZ8aptMUu1CUFK?cluster=devnet) |
| `end_round` (VRF request, ER) | [3WA1HHFm…G449](https://explorer.solana.com/tx/3WA1HHFmAgnGajWzr3ZK7mgreXMKmaZJf3gcyjMvM7WNJwpcRx5UMpvBwYUJ7E6xRizUWQfZpzPUaJkxuiwSG449?cluster=devnet) |
| `vrf_callback` (loot reveal, ER) | [58uwgMCQ…VaQq](https://explorer.solana.com/tx/58uwgMCQofabT3KHmzQ1aE3PpmtSMgvedNEgHZ7nY5ck2UmwsXzu3e2HpzZ1HUgVX1kxdGJBu45ksEovR39bVaQq?cluster=devnet) — VRF program invoked `VrfCallback`, 2,672 CU, 2 slots after the request |
| `commit_and_undelegate` (ER) | [67fnMvVv…AS2A](https://explorer.solana.com/tx/67fnMvVvrYZ9DVLMyFL77mRTMYfQDweUATrehX5RmE9YBJVdjL9uPE6gNaayFiXzfDPv7Wph7f2xngbp6mvdAS2A?cluster=devnet) |
| `claim_reward` (12 RUSH minted) | [nDrhdZwB…4mgN](https://explorer.solana.com/tx/nDrhdZwBcCkqypapQ2Y4Ncfmvvv4Zs354tXP2TN5M4EkEvsa4ThpUQKaWPe8jHZbiFvbwoQifRbHqZxy4hk4mgN?cluster=devnet) |

---

## Known limitations

- **Bot taps:** 1 tap/slot at 10 ms slots = up to 100 taps/s for a bot (human: 5–10/s). Bounded by the 30 s window and the 1000 RUSH/round cap; RUSH is a worthless devnet demo token. Accepted as designed.
- **VRF callback arena has no seeds constraint:** not exploitable — the callback is invoked only by the VRF program, with the arena from the request's `accounts_metas` (set by `end_round` to the seeds-validated arena), and the nonce binding ties it to the round.
- **VRF request origin is not authenticated (audit M-2, honest property):** `vrf_callback` checks only state==ROLLING and nonce==pending_nonce, and `pending_nonce` is public — so any third party can bind their own VRF request to a ROLLING arena and resolve it. Impact is low: the outcome stays oracle-uniform either way (the VRF program signs the randomness, not the requester), the round resolves exactly once, and the reward still goes to the arena's player. The full-width `caller_seed` (player+arena+nonce bytes) makes accidental cross-player request collisions impossible but does NOT cryptographically prevent this — by design, the callback authenticates the VRF program signer, not the request origin.
- **Live RUSH mint keeps its freeze authority:** the audit-fix build creates new mints with no freeze authority (L-4), but mint fields are immutable after creation — the live devnet mint (`6wVRct…mvYmy`) still has freeze authority = mint authority PDA. No instruction ever freezes; the authority is dormant.
- **ER Clock slot semantics + `DelegateConfig::default()` on real devnet-as: CONFIRMED live 2026-11-20** — delegation, taps (1-tap/slot cap enforced via `lastTapSlot`), VRF callback, commit and undelegation all ran on live devnet-as in the proof run above.
- **Latency:** client-measured tap→finalized was ~520 ms from this VM (egress-proxy dominated); the ER's own 10 ms blocks were confirmed via slot spacing (taps ~78 slots apart = pacing, not ER latency). A real browser will be much faster.

---

## Links

- **Repo:** https://github.com/arefki/arena-rush
- **Live demo:** https://arefki.github.io/arena-rush/
- **Demo video:** https://www.youtube.com/watch?v=lXy6oL0YaA0
- **MagicBlock docs:** https://docs.magicblock.gg
- **Solana Blitz v10:** https://luma.com/hedv5sgh · submit at https://build.magicblock.app/?stage=blitz#submit

Built for Solana Blitz v10 (Entertainment Finance) by **arefki** — X [@icibooz](https://x.com/Icibooz), Telegram [@whoism1](https://t.me/whoism1).
