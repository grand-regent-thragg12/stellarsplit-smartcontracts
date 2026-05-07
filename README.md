# StellarSplit Smart Contracts

![Build Status](https://img.shields.io/badge/build-passing-brightgreen) ![License](https://img.shields.io/badge/license-MIT-blue) ![Network](https://img.shields.io/badge/network-Stellar-7B2FBE) ![Runtime](https://img.shields.io/badge/runtime-Soroban-orange)

A trustless bill splitting and payment pooling platform built on Stellar. StellarSplit lets groups create shared payment pools, split expenses proportionally or equally, and auto-settle in USDC or XLM once all members confirm their share — no intermediaries, no trust required.

---

## Table of Contents

- [Overview](#overview)
- [Architecture](#architecture)
- [Tech Stack](#tech-stack)
- [Prerequisites](#prerequisites)
- [Installation & Setup](#installation--setup)
- [Contract Reference](#contract-reference)
  - [SplitPool](#splitpool)
  - [Settlement](#settlement)
  - [TokenRouter](#tokenrouter)
- [Deployment](#deployment)
- [Testing](#testing)
- [Environment Variables](#environment-variables)
- [Contributing](#contributing)
- [License](#license)

---

## Overview

Splitting bills in a group is a coordination problem. Someone pays upfront, others owe them, and collecting is awkward. Existing solutions require trusting a centralized platform with your funds or manually tracking who paid what.

StellarSplit solves this with on-chain payment pools. A group member creates a pool, defines the split (equal or weighted), and each participant deposits their share directly into the contract. Once all shares are confirmed and deposited, the contract automatically routes the full amount to the intended recipient — atomically, on-chain, with no manual reconciliation.

Key properties:
- Non-custodial: funds are held in the contract, not by any user or company
- Atomic settlement: payment only releases when all conditions are met
- Multi-token: supports both native XLM and USDC (or any SEP-41 compliant token)
- Transparent: all pool state is queryable on-chain

---

## Architecture

StellarSplit is composed of three Soroban smart contracts that work together:

```
┌─────────────────────────────────────────────────────────┐
│                        Client                           │
└────────────────────────┬────────────────────────────────┘
                         │
              ┌──────────▼──────────┐
              │      SplitPool      │  ← Pool creation, member
              │                     │    management, share tracking
              └──────────┬──────────┘
                         │
              ┌──────────▼──────────┐
              │     Settlement      │  ← Confirmation logic,
              │                     │    release conditions
              └──────────┬──────────┘
                         │
              ┌──────────▼──────────┐
              │    TokenRouter      │  ← Token transfers, XLM/USDC
              │                     │    routing, SAC integration
              └─────────────────────┘
```

### Contract Responsibilities

| Contract | Responsibility |
|---|---|
| `SplitPool` | Creates and manages group pools, tracks member shares and deposit status |
| `Settlement` | Validates confirmation quorum and triggers atomic fund release |
| `TokenRouter` | Abstracts token transfers, handles XLM wrapping and USDC routing via SAC |

---

## Tech Stack

| Layer | Technology |
|---|---|
| Smart Contract Runtime | [Stellar Soroban](https://soroban.stellar.org) |
| Contract Language | [Rust](https://www.rust-lang.org/) |
| Compilation Target | `wasm32-unknown-unknown` (WASM) |
| Token Standard | SEP-41 (Stellar Asset Contract) |
| CLI Tooling | [Stellar CLI](https://github.com/stellar/stellar-cli) |
| Testing Framework | Soroban SDK test utilities + Rust `#[test]` |

---

## Prerequisites

Ensure the following are installed before proceeding:

- **Rust** (stable toolchain, 1.74+)

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

- **wasm32 compilation target**

```bash
rustup target add wasm32-unknown-unknown
```

- **Stellar CLI** (v21+)

```bash
cargo install --locked stellar-cli --features opt
```

- **soroban-sdk** is pulled automatically via `Cargo.toml` — no separate install needed.

Verify your setup:

```bash
stellar --version
rustc --version
cargo --version
```

---

## Installation & Setup

1. Clone the repository:

```bash
git clone https://github.com/your-org/stellarsplit-contracts.git
cd stellarsplit-contracts
```

2. Install Rust dependencies:

```bash
cargo fetch
```

3. Build all contracts:

```bash
cargo build --target wasm32-unknown-unknown --release
```

Compiled WASM artifacts will be output to:

```
target/wasm32-unknown-unknown/release/
├── split_pool.wasm
├── settlement.wasm
└── token_router.wasm
```

4. Optimize WASM binaries (recommended before deployment):

```bash
stellar contract optimize --wasm target/wasm32-unknown-unknown/release/split_pool.wasm
stellar contract optimize --wasm target/wasm32-unknown-unknown/release/settlement.wasm
stellar contract optimize --wasm target/wasm32-unknown-unknown/release/token_router.wasm
```

---

## Contract Reference

### SplitPool

Manages the lifecycle of a payment pool. A pool has a creator, a set of members, a total amount, and a token denomination. Each member is assigned a share (in basis points or absolute amount). The pool tracks deposit status per member.

#### Key Functions

```rust
// Create a new payment pool
fn create_pool(
    env: Env,
    creator: Address,
    members: Vec<Address>,
    shares: Vec<u64>,       // share amounts in stroops or token units
    token: Address,         // token contract address (USDC or XLM SAC)
    recipient: Address,     // final payment destination
    expiry: u64,            // Unix timestamp — pool expires if not settled
) -> BytesN<32>             // returns pool_id
```

```rust
// Member deposits their share into the pool
fn deposit(
    env: Env,
    pool_id: BytesN<32>,
    member: Address,
) -> bool
```

```rust
// Query current pool state
fn get_pool(
    env: Env,
    pool_id: BytesN<32>,
) -> PoolState
```

```rust
// Cancel a pool and refund all deposited shares (creator only, before settlement)
fn cancel_pool(
    env: Env,
    pool_id: BytesN<32>,
    creator: Address,
)
```

#### PoolState struct

```rust
pub struct PoolState {
    pub creator: Address,
    pub members: Vec<Address>,
    pub shares: Map<Address, u64>,
    pub deposited: Map<Address, bool>,
    pub token: Address,
    pub recipient: Address,
    pub total_amount: u64,
    pub settled: bool,
    pub expiry: u64,
}
```

---

### Settlement

Monitors pool deposit completion and executes atomic settlement. When all members have deposited their shares, any member (or an automated keeper) can invoke `settle`. The contract verifies quorum, then calls `TokenRouter` to release funds to the recipient.

#### Key Functions

```rust
// Trigger settlement — callable by any member once all shares are deposited
fn settle(
    env: Env,
    pool_id: BytesN<32>,
) -> bool
```

```rust
// Check if a pool is ready to settle (all members deposited)
fn is_ready(
    env: Env,
    pool_id: BytesN<32>,
) -> bool
```

```rust
// Claim refund for an expired, unsettled pool
fn claim_refund(
    env: Env,
    pool_id: BytesN<32>,
    member: Address,
) -> u64   // returns refunded amount
```

Settlement emits a `PoolSettled` contract event on success, and a `PoolExpired` event when a refund is claimed past expiry.

---

### TokenRouter

Abstracts all token movement. Handles the difference between native XLM (via the Stellar Asset Contract wrapper) and USDC or other SEP-41 tokens. All transfers in SplitPool and Settlement go through TokenRouter — no contract calls token contracts directly.

#### Key Functions

```rust
// Initialize router with supported token addresses
fn initialize(
    env: Env,
    admin: Address,
    xlm_sac: Address,       // XLM Stellar Asset Contract address
    usdc_contract: Address, // USDC contract address
)
```

```rust
// Transfer tokens from a user into a pool escrow
fn transfer_in(
    env: Env,
    from: Address,
    pool_id: BytesN<32>,
    token: Address,
    amount: u64,
)
```

```rust
// Release escrowed funds to recipient (called by Settlement only)
fn release(
    env: Env,
    pool_id: BytesN<32>,
    recipient: Address,
    token: Address,
    amount: u64,
)
```

```rust
// Refund a specific member's deposit (called by Settlement on expiry)
fn refund(
    env: Env,
    pool_id: BytesN<32>,
    member: Address,
    token: Address,
    amount: u64,
)
```

---

## Deployment

### Configure your identity

```bash
stellar keys generate --global deployer --network testnet
stellar keys address deployer
```

Fund the testnet account via Friendbot:

```bash
stellar network use testnet
curl "https://friendbot.stellar.org?addr=$(stellar keys address deployer)"
```

### Testnet Deployment

Deploy contracts in dependency order: TokenRouter first, then SplitPool and Settlement.

```bash
# Deploy TokenRouter
stellar contract deploy \
  --wasm target/wasm32-unknown-unknown/release/token_router.optimized.wasm \
  --source deployer \
  --network testnet \
  -- \
  --admin $ADMIN_ADDRESS \
  --xlm_sac $XLM_SAC_ADDRESS \
  --usdc_contract $USDC_CONTRACT_ADDRESS

# Deploy SplitPool
stellar contract deploy \
  --wasm target/wasm32-unknown-unknown/release/split_pool.optimized.wasm \
  --source deployer \
  --network testnet \
  -- \
  --token_router $TOKEN_ROUTER_ADDRESS

# Deploy Settlement
stellar contract deploy \
  --wasm target/wasm32-unknown-unknown/release/settlement.optimized.wasm \
  --source deployer \
  --network testnet \
  -- \
  --split_pool $SPLIT_POOL_ADDRESS \
  --token_router $TOKEN_ROUTER_ADDRESS
```

Save the returned contract IDs — you'll need them for the environment config.

### Mainnet Deployment

```bash
# Switch to mainnet
stellar network use mainnet

# Deploy TokenRouter
stellar contract deploy \
  --wasm target/wasm32-unknown-unknown/release/token_router.optimized.wasm \
  --source deployer \
  --network mainnet \
  -- \
  --admin $ADMIN_ADDRESS \
  --xlm_sac $XLM_SAC_ADDRESS \
  --usdc_contract $USDC_CONTRACT_ADDRESS

# Deploy SplitPool
stellar contract deploy \
  --wasm target/wasm32-unknown-unknown/release/split_pool.optimized.wasm \
  --source deployer \
  --network mainnet \
  -- \
  --token_router $TOKEN_ROUTER_ADDRESS

# Deploy Settlement
stellar contract deploy \
  --wasm target/wasm32-unknown-unknown/release/settlement.optimized.wasm \
  --source deployer \
  --network mainnet \
  -- \
  --split_pool $SPLIT_POOL_ADDRESS \
  --token_router $TOKEN_ROUTER_ADDRESS
```

> **Note:** Mainnet deployments are irreversible. Verify all contract addresses and parameters before submitting. Audit the contracts before any mainnet deployment.

---

## Testing

### Unit Tests

Each contract has unit tests colocated in its `src/` directory using the Soroban SDK test environment.

```bash
# Run all unit tests
cargo test

# Run tests for a specific contract
cargo test -p split_pool
cargo test -p settlement
cargo test -p token_router

# Run with output for debugging
cargo test -- --nocapture
```

### Integration Tests

Integration tests spin up a local Soroban environment and test cross-contract interactions end-to-end.

```bash
# Run integration tests
cargo test --test integration

# Run a specific integration test
cargo test --test integration test_full_pool_settlement
```

### Test Against Testnet

To run tests against a live testnet environment, set your environment variables (see below) and use:

```bash
STELLAR_NETWORK=testnet cargo test --test e2e
```

### Coverage

```bash
cargo install cargo-tarpaulin
cargo tarpaulin --out Html --output-dir coverage/
```

---

## Environment Variables

Create a `.env` file in the project root (never commit this file):

```bash
cp .env.example .env
```

| Variable | Description | Example |
|---|---|---|
| `STELLAR_NETWORK` | Target network (`testnet` or `mainnet`) | `testnet` |
| `STELLAR_RPC_URL` | Soroban RPC endpoint | `https://soroban-testnet.stellar.org` |
| `STELLAR_NETWORK_PASSPHRASE` | Network passphrase for signing | `Test SDF Network ; September 2015` |
| `ADMIN_ADDRESS` | Stellar address of the contract admin | `GABC...XYZ` |
| `DEPLOYER_SECRET` | Secret key of the deploying account | `SABC...XYZ` |
| `USDC_CONTRACT_ADDRESS` | Contract ID of the USDC SAC | `CCBC...XYZ` |
| `XLM_SAC_ADDRESS` | Contract ID of the native XLM SAC | `CDLZ...XYZ` |
| `SPLIT_POOL_ADDRESS` | Deployed SplitPool contract ID | `CDEF...XYZ` |
| `SETTLEMENT_ADDRESS` | Deployed Settlement contract ID | `CGHI...XYZ` |
| `TOKEN_ROUTER_ADDRESS` | Deployed TokenRouter contract ID | `CJKL...XYZ` |

> **Security:** Never commit `.env` or any file containing secret keys. Add `.env` to `.gitignore`.

---

## Contributing

Contributions are welcome. Please follow these guidelines:

1. Fork the repository and create a feature branch from `main`:

```bash
git checkout -b feat/your-feature-name
```

2. Write tests for any new contract logic. PRs without test coverage will not be merged.

3. Ensure all tests pass and the build is clean before opening a PR:

```bash
cargo test
cargo clippy -- -D warnings
cargo fmt --check
```

4. Open a pull request against `main` with a clear description of the change and its motivation.

5. For significant changes (new contracts, breaking interface changes), open an issue first to discuss the approach.

### Code Style

- Follow standard Rust formatting (`cargo fmt`)
- No `clippy` warnings allowed in CI
- Contract functions must have doc comments explaining parameters and return values
- Avoid `unwrap()` in contract code — use proper error types

---

## License

This project is licensed under the MIT License. See [LICENSE](./LICENSE) for the full text.

---

*Built on [Stellar](https://stellar.org) · Powered by [Soroban](https://soroban.stellar.org)*
