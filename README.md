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
| Program build | ✅ Built — `anchor-1.0.2 build` green, `.so` 394,680 bytes |
| MagicSVM tests | ✅ 3/3 passing (see [Tests](#tests)) |
| Frontend | ✅ Built — static site in `docs/`, smoke-tested headless |
| **Devnet deploy** | ✅ **LIVE on devnet since 2026-11-20 11:56 WIB** — [deploy tx](https://explorer.solana.com/tx/m9oodwv3NgknwpNjZkHGVem1nA1epL31ZruACKnT7vsR19U6GNnnUTq12jj9y3cTWwa8SZvt8UGL86VANeHJUb2?cluster=devnet) (slot 524237385) |
| **Live proof run** | ✅ **Full loop verified live 2026-11-20 11:57–11:59 WIB** — delegate → 6 taps → VRF Epic ×3 → commit → claim 18 RUSH (see [Live proof](#live-proof)) |

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

The program is **live on devnet**. The hosted demo link goes live once the GitHub repo is created and Pages is enabled.

- **Live demo (pending Pages):** `https://arefki.github.io/arena-rush/` — *repo + Pages setup in progress; the site in `docs/` is ready to serve*
- **Demo video (pending):** *link to be added after recording*

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

**4 screens:** Lobby (status grid + Enter the Arena) → Arena (canvas, 30 s countdown, optimistic taps) → Loot chest (VRF reveal + `force_expire` fallback button after 130 s) → Results (score, multiplier, RUSH won, proof tx links).

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

Deploy needs ~2 SOL program rent (394,680-byte program; buffer rent was 2.006 SOL). ✅ **Deployed 2026-11-20 11:56 WIB** — [deploy tx](https://explorer.solana.com/tx/m9oodwv3NgknwpNjZkHGVem1nA1epL31ZruACKnT7vsR19U6GNnnUTq12jj9y3cTWwa8SZvt8UGL86VANeHJUb2?cluster=devnet). Note: the first `anchor deploy` attempt timed out mid-upload (slow RPC); the deploy was resumed with `solana program deploy --buffer <buffer-address>` and finalized in ~3 s. The IDL is also initialized on-chain.

---

## Tests

MagicSVM 0.3.0 (in-process base + ephemeral ledgers, incl. VRF callbacks) — no local validator needed.

```bash
cargo +stable test -p arena-rush-magicsvm-tests   # ~4.5 s
```

**Result: 3/3 passing** (verified 2026-11-20):

| Test | Covers |
|---|---|
| `full_lifecycle` | init → delegate → 5 taps → VRF resolve → commit → claim; RUSH minted = score × multiplier; second claim rejected |
| `guards` | tap / end_round / commit / claim before their states fail; target replay rejected; force_expire while PLAYING rejected |
| `force_expire_recovers_stuck_round` | 130 s timeout → Common ×1; late VRF callback afterwards rejected; commit + claim still work |

MagicSVM caught a real bug the build would otherwise have shipped: the `#[vrf_callback]` macro's runtime PDA bump search consumed the entire ~200k CU budget before the handler ran. Fixed by declaring the scoped VRF identity explicitly (`.so` shrank 425,664 → 395,352 bytes). This would have failed on real devnet too — the VRF oracle's fulfillment tx has the same budget.

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

---

## Known limitations

- **Bot taps:** 1 tap/slot at 10 ms slots = up to 100 taps/s for a bot (human: 5–10/s). Bounded by the 30 s window and the 1000 RUSH/round cap; RUSH is a worthless devnet demo token. Accepted as designed.
- **VRF callback arena has no seeds constraint:** not exploitable — the callback is invoked only by the VRF program, with the arena from the request's `accounts_metas` (set by `end_round` to the seeds-validated arena), and the nonce binding ties it to the round.
- **ER Clock slot semantics + `DelegateConfig::default()` on real devnet-as: CONFIRMED live 2026-11-20** — delegation, taps (1-tap/slot cap enforced via `lastTapSlot`), VRF callback, commit and undelegation all ran on live devnet-as in the proof run above.
- **Latency:** client-measured tap→finalized was ~520 ms from this VM (egress-proxy dominated); the ER's own 10 ms blocks were confirmed via slot spacing (taps ~78 slots apart = pacing, not ER latency). A real browser will be much faster.

---

## Links

- **Repo:** https://github.com/arefki/arena-rush — *⏳ repo to be created; local repo ready to push*
- **Live demo:** _pending deploy + GitHub Pages_
- **Demo video:** _pending recording_
- **MagicBlock docs:** https://docs.magicblock.gg
- **Solana Blitz v10:** https://luma.com/hedv5sgh · submit at https://build.magicblock.app/?stage=blitz#submit

Built for Solana Blitz v10 (Entertainment Finance) by **arefki** — X [@icibooz](https://x.com/Icibooz), Telegram [@whoism1](https://t.me/whoism1).
