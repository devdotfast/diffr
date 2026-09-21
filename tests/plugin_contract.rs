// Both crates ship the contract so they can build independently from crates.io.
#[test]
fn packaged_plugin_contract_matches_host() {
    assert_eq!(
        include_str!("../wit/plugin.wit"),
        include_str!("../crates/diffr-plugin-sdk/wit/plugin.wit"),
    );
}
