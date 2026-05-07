#![no_std]

use soroban_sdk::{
    contract, contractimpl, contracttype, vec, Address, BytesN, Env, Map, Symbol, Vec,
};

// ── Types ─────────────────────────────────────────────────────────────────────

/// On-chain state for a single payment pool.
#[contracttype]
#[derive(Clone)]
pub struct PoolState {
    pub creator: Address,
    pub members: Vec<Address>,
    pub shares: Map<Address, i128>,
    pub deposited: Map<Address, bool>,
    pub token: Address,
    pub recipient: Address,
    pub total_amount: i128,
    pub settled: bool,
    pub expiry: u64,
}

#[contracttype]
pub enum DataKey {
    Pool(BytesN<32>),
    TokenRouter,
    PoolCount,
}

// ── Events ────────────────────────────────────────────────────────────────────

fn emit_pool_created(env: &Env, pool_id: &BytesN<32>, creator: &Address) {
    env.events().publish(
        (Symbol::new(env, "pool_created"), pool_id.clone()),
        creator.clone(),
    );
}

fn emit_deposit(env: &Env, pool_id: &BytesN<32>, member: &Address, amount: i128) {
    env.events().publish(
        (Symbol::new(env, "deposit"), pool_id.clone()),
        (member.clone(), amount),
    );
}

fn emit_pool_cancelled(env: &Env, pool_id: &BytesN<32>) {
    env.events()
        .publish((Symbol::new(env, "pool_cancelled"), pool_id.clone()), ());
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn load_pool(env: &Env, pool_id: &BytesN<32>) -> PoolState {
    env.storage()
        .persistent()
        .get(&DataKey::Pool(pool_id.clone()))
        .expect("pool not found")
}

fn save_pool(env: &Env, pool_id: &BytesN<32>, pool: &PoolState) {
    env.storage()
        .persistent()
        .set(&DataKey::Pool(pool_id.clone()), pool);
}

/// Derive a deterministic pool ID from the ledger sequence and a counter.
fn next_pool_id(env: &Env) -> BytesN<32> {
    let count: u64 = env
        .storage()
        .instance()
        .get(&DataKey::PoolCount)
        .unwrap_or(0);
    env.storage()
        .instance()
        .set(&DataKey::PoolCount, &(count + 1));

    let mut seed = [0u8; 32];
    let seq = env.ledger().sequence();
    seed[..8].copy_from_slice(&seq.to_be_bytes());
    seed[8..16].copy_from_slice(&count.to_be_bytes());
    BytesN::from_array(env, &seed)
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct SplitPool;

#[contractimpl]
impl SplitPool {
    /// Set the TokenRouter contract address (admin setup, call once after deploy).
    pub fn set_token_router(env: Env, admin: Address, token_router: Address) {
        admin.require_auth();
        if env.storage().instance().has(&DataKey::TokenRouter) {
            panic!("token_router already set");
        }
        env.storage()
            .instance()
            .set(&DataKey::TokenRouter, &token_router);
    }

    /// Create a new payment pool.
    ///
    /// - `members` and `shares` must be the same length.
    /// - `shares` are absolute token amounts (in stroops / token base units).
    /// - `expiry` is a Unix timestamp; the pool auto-expires after this time.
    pub fn create_pool(
        env: Env,
        creator: Address,
        members: Vec<Address>,
        shares: Vec<i128>,
        token: Address,
        recipient: Address,
        expiry: u64,
    ) -> BytesN<32> {
        creator.require_auth();

        assert!(members.len() > 0, "at least one member required");
        assert_eq!(members.len(), shares.len(), "members and shares length mismatch");
        assert!(expiry > env.ledger().timestamp(), "expiry must be in the future");

        let mut share_map: Map<Address, i128> = Map::new(&env);
        let mut deposited_map: Map<Address, bool> = Map::new(&env);
        let mut total: i128 = 0;

        for i in 0..members.len() {
            let member = members.get(i).unwrap();
            let share = shares.get(i).unwrap();
            assert!(share > 0, "share must be positive");
            share_map.set(member.clone(), share);
            deposited_map.set(member, false);
            total += share;
        }

        let pool_id = next_pool_id(&env);
        let pool = PoolState {
            creator: creator.clone(),
            members,
            shares: share_map,
            deposited: deposited_map,
            token,
            recipient,
            total_amount: total,
            settled: false,
            expiry,
        };

        save_pool(&env, &pool_id, &pool);
        emit_pool_created(&env, &pool_id, &creator);
        pool_id
    }

    /// Record a member's deposit. Calls TokenRouter to pull funds into escrow.
    pub fn deposit(env: Env, pool_id: BytesN<32>, member: Address) -> bool {
        member.require_auth();

        let mut pool = load_pool(&env, &pool_id);
        assert!(!pool.settled, "pool already settled");
        assert!(
            env.ledger().timestamp() < pool.expiry,
            "pool has expired"
        );

        let already = pool.deposited.get(member.clone()).unwrap_or(false);
        assert!(!already, "member already deposited");

        let share = pool
            .shares
            .get(member.clone())
            .expect("caller is not a pool member");

        // Call TokenRouter to pull the member's share into escrow
        let router: Address = env
            .storage()
            .instance()
            .get(&DataKey::TokenRouter)
            .expect("token_router not configured");

        token_router::TokenRouterClient::new(&env, &router).transfer_in(
            &member,
            &pool_id,
            &pool.token,
            &share,
        );

        pool.deposited.set(member.clone(), true);
        save_pool(&env, &pool_id, &pool);
        emit_deposit(&env, &pool_id, &member, share);

        // Return true if all members have now deposited
        Self::all_deposited_internal(&pool)
    }

    /// Cancel a pool and trigger refunds for all deposited members.
    /// Only the creator can cancel, and only before settlement.
    pub fn cancel_pool(env: Env, pool_id: BytesN<32>, creator: Address) {
        creator.require_auth();

        let mut pool = load_pool(&env, &pool_id);
        assert_eq!(pool.creator, creator, "only creator can cancel");
        assert!(!pool.settled, "pool already settled");

        let router: Address = env
            .storage()
            .instance()
            .get(&DataKey::TokenRouter)
            .expect("token_router not configured");

        let router_client = token_router::TokenRouterClient::new(&env, &router);

        for member in pool.members.iter() {
            let has_deposited = pool.deposited.get(member.clone()).unwrap_or(false);
            if has_deposited {
                let share = pool.shares.get(member.clone()).unwrap();
                router_client.refund(
                    &env.current_contract_address(),
                    &pool_id,
                    &member,
                    &pool.token,
                    &share,
                );
                pool.deposited.set(member, false);
            }
        }

        pool.settled = true; // mark as closed to prevent further deposits
        save_pool(&env, &pool_id, &pool);
        emit_pool_cancelled(&env, &pool_id);
    }

    /// Mark a pool as settled. Called by the Settlement contract after releasing funds.
    pub fn mark_settled(env: Env, caller: Address, pool_id: BytesN<32>) {
        caller.require_auth();
        let mut pool = load_pool(&env, &pool_id);
        assert!(!pool.settled, "already settled");
        pool.settled = true;
        save_pool(&env, &pool_id, &pool);
    }

    /// Returns true if all members have deposited their share.
    pub fn all_deposited(env: Env, pool_id: BytesN<32>) -> bool {
        let pool = load_pool(&env, &pool_id);
        Self::all_deposited_internal(&pool)
    }

    /// Query full pool state.
    pub fn get_pool(env: Env, pool_id: BytesN<32>) -> PoolState {
        load_pool(&env, &pool_id)
    }

    // ── Internal ──────────────────────────────────────────────────────────────

    fn all_deposited_internal(pool: &PoolState) -> bool {
        for member in pool.members.iter() {
            if !pool.deposited.get(member).unwrap_or(false) {
                return false;
            }
        }
        true
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{
        testutils::{Address as _, Ledger},
        vec, Env,
    };

    fn setup_env() -> (Env, Address) {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, SplitPool);
        (env, contract_id)
    }

    #[test]
    fn test_create_pool() {
        let (env, contract_id) = setup_env();
        let client = SplitPoolClient::new(&env, &contract_id);

        // Register a dummy token router
        let router = env.register_contract(None, token_router::TokenRouter);
        let router_client = token_router::TokenRouterClient::new(&env, &router);
        let admin = Address::generate(&env);
        let xlm = Address::generate(&env);
        let usdc = Address::generate(&env);
        router_client.initialize(&admin, &xlm, &usdc);
        client.set_token_router(&admin, &router);

        let creator = Address::generate(&env);
        let alice = Address::generate(&env);
        let bob = Address::generate(&env);

        env.ledger().with_mut(|l| l.timestamp = 1000);

        let pool_id = client.create_pool(
            &creator,
            &vec![&env, alice.clone(), bob.clone()],
            &vec![&env, 500_i128, 500_i128],
            &usdc,
            &creator,
            &9999,
        );

        let pool = client.get_pool(&pool_id);
        assert_eq!(pool.total_amount, 1000);
        assert!(!pool.settled);
        assert_eq!(pool.members.len(), 2);
    }

    #[test]
    #[should_panic(expected = "members and shares length mismatch")]
    fn test_create_pool_length_mismatch() {
        let (env, contract_id) = setup_env();
        let client = SplitPoolClient::new(&env, &contract_id);

        let creator = Address::generate(&env);
        let alice = Address::generate(&env);
        let token = Address::generate(&env);

        env.ledger().with_mut(|l| l.timestamp = 1000);

        client.create_pool(
            &creator,
            &vec![&env, alice],
            &vec![&env, 500_i128, 500_i128],
            &token,
            &creator,
            &9999,
        );
    }

    #[test]
    fn test_all_deposited_false_initially() {
        let (env, contract_id) = setup_env();
        let client = SplitPoolClient::new(&env, &contract_id);

        let router = env.register_contract(None, token_router::TokenRouter);
        let router_client = token_router::TokenRouterClient::new(&env, &router);
        let admin = Address::generate(&env);
        let xlm = Address::generate(&env);
        let usdc = Address::generate(&env);
        router_client.initialize(&admin, &xlm, &usdc);
        client.set_token_router(&admin, &router);

        let creator = Address::generate(&env);
        let alice = Address::generate(&env);

        env.ledger().with_mut(|l| l.timestamp = 1000);

        let pool_id = client.create_pool(
            &creator,
            &vec![&env, alice],
            &vec![&env, 1000_i128],
            &usdc,
            &creator,
            &9999,
        );

        assert!(!client.all_deposited(&pool_id));
    }
}
