use alloy::sol;

sol!(
    #[sol(rpc, all_derives)]
    ICS26Router,
    "../../../external/solidity-ibc-eureka/abi/ICS26Router.json"
);
