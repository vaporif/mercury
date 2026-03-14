pub mod ics26_router {
    alloy::sol!(
        #[sol(rpc, all_derives)]
        ICS26Router,
        "../../../external/solidity-ibc-eureka/abi/ICS26Router.json"
    );
}

pub mod sp1_ics07 {
    alloy::sol!(
        #[sol(rpc, all_derives)]
        SP1ICS07Tendermint,
        "../../../external/solidity-ibc-eureka/abi/SP1ICS07Tendermint.json"
    );
}

// Re-export commonly used types for convenience.
pub use ics26_router::{ICS26Router, IICS02ClientMsgs, IICS26RouterMsgs};
pub use sp1_ics07::SP1ICS07Tendermint;

#[test]
fn test_update_and_migrate_calls_exist() {
    let _ = core::mem::size_of::<ICS26Router::updateClientCall>();
    let _ = core::mem::size_of::<ICS26Router::migrateClientCall>();
}
