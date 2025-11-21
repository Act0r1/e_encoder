use alloy::primitives::{Address, Bytes as AlloyBytes, Keccak256, U256};
use alloy::sol;
use alloy::sol_types::SolCall;
use anyhow::Result;

pub fn encode_input(selector: &str, mut encoded_args: Vec<u8>) -> Vec<u8> {
    let mut hasher = Keccak256::new();
    hasher.update(selector.as_bytes());
    let selector_bytes = &hasher.finalize()[..4];
    let mut call_data = selector_bytes.to_vec();

    // Remove extra prefix if present (32 bytes for dynamic data)
    // Alloy encoding is including a prefix for dynamic data indicating the offset or length
    // but at this point we don't want that
    if encoded_args.len() > 32
        && encoded_args[..32]
            == [0u8; 31]
                .into_iter()
                .chain([32].to_vec())
                .collect::<Vec<u8>>()
    {
        encoded_args = encoded_args[32..].to_vec();
    }

    call_data.extend(encoded_args);
    call_data
}

pub fn create_multitrade_calldata(
    token_address: Address,
    executor_address: Address,
    approve_calldata: Vec<u8>,
    swap_calldata: Vec<u8>,
) -> Result<Vec<u8>> {
    sol!(
        struct Data {
            address target;
            uint256 value;
            bytes callData;
        }
       #[sol(rpc)]
       function executeInteractions(Data[] interactions, address tokenAddress, uint8 isTest) external payable;
    );

    let interactions = vec![
        Data {
            target: token_address,
            value: U256::ZERO,
            callData: AlloyBytes::from(approve_calldata),
        },
        Data {
            target: executor_address,
            value: U256::ZERO,
            callData: AlloyBytes::from(swap_calldata),
        },
    ];

    let args = executeInteractionsCall::new((interactions, token_address, 1u8));
    let encoded_args = args.abi_encode();

    Ok(encoded_args)
}
