// Action CI fixture: an unguarded privileged state write, which AUTH-001
// reports as a critical finding. Used by sdkt-action-ci.yml to assert that
// `sdkt audit --fail-on critical` fails the gate.
#[contractimpl]
impl Token {
    pub fn mint(env: Env, to: Address) {
        env.storage().instance().set(&K, &to);
    }
}
