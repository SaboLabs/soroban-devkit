// Action CI fixture: a guarded, side-effect-free getter, which the audit
// reports with no findings. Used by sdkt-action-ci.yml to assert that
// `sdkt audit --fail-on critical` passes the gate on a clean report.
#[contractimpl]
impl Token {
    pub fn balance_of(env: Env, who: Address) -> i128 {
        who.require_auth();
        0
    }
}
