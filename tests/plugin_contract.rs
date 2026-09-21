// Keep the independently packaged contracts identical.
#[test]
fn packaged_plugin_contract_matches_host() {
    assert_eq!(
        include_str!("../wit/plugin.wit"),
        include_str!("../crates/diffr-plugin-sdk/wit/plugin.wit"),
    );
}
