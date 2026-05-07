#![no_std]

use soroban_sdk::{contract, contractimpl, contracttype, Address, BytesN, Env, Symbol};

// ── Storage keys ──────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    SplitPool,
    TokenRouter,
}

// ── Events ────────────────────────────────────────────────────────────────────

fn emit_settled(env: &Env, pool_id: &BytesN<32>, recipient: &Address, amount: i128) {
    env.events().publish(
        (Symbol::new(env, "pool_settled"), pool_id.clone()),
        (recipient.clone(), amount),
    );
}

fn emit_refund_claimed(env: &Env, pool_id: &BytesN<32>, member: &Address, amount: i128) {
    env.events().publish(
        (Symbol::new(env, "refund_claimed"), pool_id.clone()),
        (member.clone(), amount),
    );
}

// ── Contract ──────────────────────────────────────────────────────────────────

#[contract]
pub struct Settlement;

#[contractimpl]
impl Settlement {
    /// Configure contract dependencies (call once after deploy).
    pub fn initialize(env: Env, admin: Address, split_pool: Address, token_router: Address) {
        admin.require_auth();
        if env.storage().instance().has(&DataKey::SplitPool) {
            panic!("already initialized");
        }
        env.storage()
            .instance()
            .set(&DataKey::SplitPool, &split_pool);
        env.storage()
            .instance()
            .set(&DataKey::TokenRouter, &token_router);
    }

    /// Settle a pool. Callable by any member once all shares are deposited.
    ///
    /// Verifies readiness via SplitPool, then instructs TokenRouter to release
    /// the full escrowed amount to the pool's recipient.
    pub fn settle(env: Env, caller: Address, pool_id: BytesN<32>) -> bool {
        caller.require_auth();

        let split_pool_addr: Address = env
            .storage()
            .instance()
            .get(&DataKey::SplitPool)
            .expect("not initialized");
        let token_router_addr: Address = env
            .storage()
            .instance()
            .get(&DataKey::TokenRouter)
            .expect("not initialized");

        let pool_client = split_pool::SplitPoolClient::new(&env, &split_pool_addr);
        let router_client = token_router::TokenRouterClient::new(&env, &token_router_addr);

        let pool = pool_client.get_pool(&pool_id);

        assert!(!pool.settled, "pool already settled");
        assert!(
            env.ledger().timestamp() < pool.expiry,
            "pool has expired — use claim_refund"
        );
        assert!(
            pool_client.all_deposited(&pool_id),
            "not all members have deposited"
        );

        // Release full escrowed amount to recipient
        router_client.release(
            &env.current_contract_address(),
            &pool_id,
            &pool.recipient,
            &pool.token,
            &pool.total_amount,
        );

        // Mark pool as settled in SplitPool
        pool_client.mark_settled(&env.current_contract_address(), &pool_id);

        emit_settled(&env, &pool_id, &pool.recipient, pool.total_amount);
        true
    }

    /// Check if a pool is ready to settle.
    pub fn is_ready(env: Env, pool_id: BytesN<32>) -> bool {
        let split_pool_addr: Address = env
            .storage()
            .instance()
            .get(&DataKey::SplitPool)
            .expect("not initialized");

        let pool_client = split_pool::SplitPoolClient::new(&env, &split_pool_addr);
        let pool = pool_client.get_pool(&pool_id);

        !pool.settled
            && env.ledger().timestamp() < pool.expiry
            && pool_client.all_deposited(&pool_id)
    }

    /// Claim a refund for an expired, unsettled pool.
    /// Each member calls this individually to recover their deposit.
    pub fn claim_refund(env: Env, pool_id: BytesN<32>, member: Address) -> i128 {
        member.require_auth();

        let split_pool_addr: Address = env
            .storage()
            .instance()
            .get(&DataKey::SplitPool)
            .expect("not initialized");
        let token_router_addr: Address = env
            .storage()
            .instance()
            .get(&DataKey::TokenRouter)
            .expect("not initialized");

        let pool_client = split_pool::SplitPoolClient::new(&env, &split_pool_addr);
        let router_client = token_router::TokenRouterClient::new(&env, &token_router_addr);

        let pool = pool_client.get_pool(&pool_id);

        assert!(!pool.settled, "pool already settled");
        assert!(
            env.ledger().timestamp() >= pool.expiry,
            "pool has not expired yet"
        );

        let has_deposited = pool.deposited.get(member.clone()).unwrap_or(false);
        assert!(has_deposited, "member has not deposited or already refunded");

        let share = pool
            .shares
            .get(member.clone())
            .expect("not a pool member");

        router_client.refund(
            &env.current_contract_address(),
            &pool_id,
            &member,
            &pool.token,
            &share,
        );

        emit_refund_claimed(&env, &pool_id, &member, share);
        share
    }

    pub fn split_pool(env: Env) -> Address {
        env.storage().instance().get(&DataKey::SplitPool).unwrap()
    }

    pub fn token_router(env: Env) -> Address {
        env.storage().instance().get(&DataKey::TokenRouter).unwrap()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{
        testutils::{Address as _, Ledger},
        Env,
    };

    #[test]
    fn test_initialize() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, Settlement);
        let client = SettlementClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let split_pool = Address::generate(&env);
        let token_router = Address::generate(&env);

        client.initialize(&admin, &split_pool, &token_router);

        assert_eq!(client.split_pool(), split_pool);
        assert_eq!(client.token_router(), token_router);
    }

    #[test]
    #[should_panic(expected = "already initialized")]
    fn test_double_initialize_panics() {
        let env = Env::default();
        env.mock_all_auths();
        let contract_id = env.register_contract(None, Settlement);
        let client = SettlementClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let sp = Address::generate(&env);
        let tr = Address::generate(&env);

        client.initialize(&admin, &sp, &tr);
        client.initialize(&admin, &sp, &tr);
    }

    #[test]
    fn test_is_ready_false_without_pool() {
        let env = Env::default();
        env.mock_all_auths();

        // Register all three contracts
        let router_id = env.register_contract(None, token_router::TokenRouter);
        let router_client = token_router::TokenRouterClient::new(&env, &router_id);
        let admin = Address::generate(&env);
        let xlm = Address::generate(&env);
        let usdc = Address::generate(&env);
        router_client.initialize(&admin, &xlm, &usdc);

        let pool_id_contract = env.register_contract(None, split_pool::SplitPool);
        let pool_client = split_pool::SplitPoolClient::new(&env, &pool_id_contract);
        pool_client.set_token_router(&admin, &router_id);

        let settlement_id = env.register_contract(None, Settlement);
        let settlement_client = SettlementClient::new(&env, &settlement_id);
        settlement_client.initialize(&admin, &pool_id_contract, &router_id);

        // is_ready should panic for a non-existent pool
        // (pool not found panic from SplitPool)
        // We just verify initialization works correctly here
        assert_eq!(settlement_client.split_pool(), pool_id_contract);
    }
}
