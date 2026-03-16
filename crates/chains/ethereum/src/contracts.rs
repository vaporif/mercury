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

pub mod ics20_transfer {
    alloy::sol!(
        #[sol(rpc, all_derives)]
        ICS20Transfer,
        "../../../external/solidity-ibc-eureka/abi/ICS20Transfer.json"
    );
}

pub mod ibc_erc20 {
    alloy::sol!(
        #[sol(rpc, all_derives)]
        IBCERC20,
        "../../../external/solidity-ibc-eureka/abi/IBCERC20.json"
    );
}

pub use ibc_erc20::IBCERC20;
pub use ics20_transfer::ICS20Transfer;
pub use ics26_router::{ICS26Router, IICS02ClientMsgs, IICS26RouterMsgs};
pub use sp1_ics07::SP1ICS07Tendermint;

#[test]
fn test_update_and_migrate_calls_exist() {
    let _ = core::mem::size_of::<ICS26Router::updateClientCall>();
    let _ = core::mem::size_of::<ICS26Router::migrateClientCall>();
}
