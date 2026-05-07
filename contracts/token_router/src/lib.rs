#![no_std]

use soroban_sdk::{
    contract, contractimpl, contracttype, token, Address, BytesN, Env, Symbol,
};

// ── Storage keys ────────────────────────────────────────────────────────────

#[contracttype]
pub enum DataKey {
    Admin,
    XlmSac,
    UsdcContract,
    /// Escrowed balance per (pool_id, token) pair
    Escrow(BytesN<32>, Address),
}

// ── Events ───────────────────────────────────────────────────────────────────

fn emit_transfer_in(env: &Env, pool_id: &BytesN<32>, from: &Address, amount: i128) {
    env.events().publish(
        (Symbol::new(env, "transfer_in"), pool_id.clone()),
        (from.clone(), amount),
    );
}

fn emit_release(env: &Env, pool_id: &BytesN<32>, recipient: &Address, amount: i128) {
    env.events().publish(
        (Symbol::new(env, "release"), pool_id.clone()),
        (recipient.clone(), amount),
    );
}

fn emit_refund(env: &Env, pool_id: &BytesN<32>, member: &Address, amount: i128) {
    env.events().publish(
        (Symbol::new(env, "refund"), pool_id.clone()),
        (member.clone(), amount),
    );
}

// ── Contract ─────────────────────────────────────────────────────────────────

#[contract]
pub struct TokenRouter;

#[contractimpl]
impl TokenRouter {
    /// Initialize the router with supported token addresses.
    pub fn initialize(
        env: Env,
        admin: Address,
        xlm_sac: Address,
        usdc_contract: Address,
    ) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic!("already initialized");
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::XlmSac, &xlm_sac);
        env.storage().instance().set(&DataKey::UsdcContract, &usdc_contract);
    }

    /// Transfer tokens from `from` into escrow for `pool_id`.
    pub fn transfer_in(
        env: Env,
        from: Address,
        pool_id: BytesN<32>,
        token: Address,
        amount: i128,
    ) {
        from.require_auth();
        assert!(amount > 0, "amount must be positive");

        let client = token::Client::new(&env, &token);
        client.transfer(&from, &env.current_contract_address(), &amount);

        let key = DataKey::Escrow(pool_id.clone(), token);
        let current: i128 = env.storage().temporary().get(&key).unwrap_or(0);
        env.storage().temporary().set(&key, &(current + amount));

        emit_transfer_in(&env, &pool_id, &from, amount);
    }

    /// Release escrowed funds to `recipient`. Only callable by the Settlement contract.
    pub fn release(
        env: Env,
        caller: Address,
        pool_id: BytesN<32>,
        recipient: Address,
        token: Address,
        amount: i128,
    ) {
        caller.require_auth();
        assert!(amount > 0, "amount must be positive");

        let key = DataKey::Escrow(pool_id.clone(), token.clone());
        let escrowed: i128 = env.storage().temporary().get(&key).unwrap_or(0);
        assert!(escrowed >= amount, "insufficient escrow balance");

        let client = token::Client::new(&env, &token);
        client.transfer(&env.current_contract_address(), &recipient, &amount);

        env.storage().temporary().set(&key, &(escrowed - amount));
        emit_release(&env, &pool_id, &recipient, amount);
    }

    /// Refund a member's deposit from escrow. Only callable by the Settlement contract.
    pub fn refund(
        env: Env,
        caller: Address,
        pool_id: BytesN<32>,
        member: Address,
        token: Address,
        amount: i128,
    ) {
        caller.require_auth();
        assert!(amount > 0, "amount must be positive");

        let key = DataKey::Escrow(pool_id.clone(), token.clone());
        let escrowed: i128 = env.storage().temporary().get(&key).unwrap_or(0);
        assert!(escrowed >= amount, "insufficient escrow balance");

        let client = token::Client::new(&env, &token);
        client.transfer(&env.current_contract_address(), &member, &amount);

        env.storage().temporary().set(&key, &(escrowed - amount));
        emit_refund(&env, &pool_id, &member, amount);
    }

    /// Query escrowed balance for a pool/token pair.
    pub fn escrow_balance(env: Env, pool_id: BytesN<32>, token: Address) -> i128 {
        let key = DataKey::Escrow(pool_id, token);
        env.storage().temporary().get(&key).unwrap_or(0)
    }

    pub fn admin(env: Env) -> Address {
        env.storage().instance().get(&DataKey::Admin).unwrap()
    }

    pub fn xlm_sac(env: Env) -> Address {
        env.storage().instance().get(&DataKey::XlmSac).unwrap()
    }

    pub fn usdc_contract(env: Env) -> Address {
        env.storage().instance().get(&DataKey::UsdcContract).unwrap()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{
        testutils::{Address as _, BytesN as _},
        Env,
    };

    #[test]
    fn test_initialize() {
        let env = Env::default();
        let contract_id = env.register_contract(None, TokenRouter);
        let client = TokenRouterClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let xlm_sac = Address::generate(&env);
        let usdc = Address::generate(&env);

        client.initialize(&admin, &xlm_sac, &usdc);

        assert_eq!(client.admin(), admin);
        assert_eq!(client.xlm_sac(), xlm_sac);
        assert_eq!(client.usdc_contract(), usdc);
    }

    #[test]
    #[should_panic(expected = "already initialized")]
    fn test_double_initialize_panics() {
        let env = Env::default();
        let contract_id = env.register_contract(None, TokenRouter);
        let client = TokenRouterClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let xlm_sac = Address::generate(&env);
        let usdc = Address::generate(&env);

        client.initialize(&admin, &xlm_sac, &usdc);
        client.initialize(&admin, &xlm_sac, &usdc);
    }

    #[test]
    fn test_escrow_balance_default_zero() {
        let env = Env::default();
        let contract_id = env.register_contract(None, TokenRouter);
        let client = TokenRouterClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let xlm_sac = Address::generate(&env);
        let usdc = Address::generate(&env);
        client.initialize(&admin, &xlm_sac, &usdc);

        let pool_id = BytesN::random(&env);
        let token = Address::generate(&env);
        assert_eq!(client.escrow_balance(&pool_id, &token), 0);
    }
}
