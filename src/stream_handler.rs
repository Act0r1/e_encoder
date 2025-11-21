use std::str::FromStr;

use alloy::hex;
use alloy::primitives::{Address, B256, address};
use alloy::signers::local::PrivateKeySigner;
use alloy::sol_types::SolValue;
use anyhow::Result;
use num_bigint::BigUint;
use tracing::info;

use tycho_execution::encoding::evm::encoder_builders::TychoRouterEncoderBuilder;
use tycho_execution::encoding::models::{Solution, Swap};
use tycho_simulation::evm::protocol::u256_num::biguint_to_u256;
use tycho_simulation::protocol::models::ProtocolComponent;
use tycho_simulation::tycho_common::hex_bytes::Bytes;
use tycho_simulation::tycho_common::models::Chain;
use tycho_simulation::tycho_common::models::token::Token;

use crate::encoding::{create_multitrade_calldata, encode_input};

pub fn process_swap(
    component: &ProtocolComponent,
    sell_token: &Token,
    buy_token: &Token,
    amount_in: BigUint,
    amount_out: BigUint,
    _rpc_url: &str,
    private_key: &str,
) -> Result<Vec<u8>> {
    info!(
        "Processing swap: {} -> {}",
        sell_token.symbol, buy_token.symbol
    );

    let encoder = TychoRouterEncoderBuilder::new()
        .user_transfer_type(tycho_execution::encoding::models::UserTransferType::TransferFrom)
        .chain(Chain::Ethereum)
        .build()?;

    let pk = B256::from_str(private_key)?;
    let signer = PrivateKeySigner::from_bytes(&pk)?;
    let slippage_tolerance = 5; // 5%
    let min_amount_out =
        amount_out.clone() * BigUint::from((100 - slippage_tolerance) as u32) / BigUint::from(100u32);

    let component_clone = component.clone().into();
    let swap = Swap {
        component: component_clone,
        token_in: Bytes::from(sell_token.address.as_ref()),
        token_out: Bytes::from(buy_token.address.as_ref()),
        split: 0.0,
        user_data: None,
        protocol_state: None,
        estimated_amount_in: Some(amount_in.clone()),
    };

    let solution = Solution {
        sender: Bytes::from(signer.address().as_slice()),
        receiver: Bytes::from(signer.address().as_slice()),
        given_token: Bytes::from(sell_token.address.as_ref()),
        given_amount: amount_in.clone(),
        checked_token: Bytes::from(buy_token.address.as_ref()),
        exact_out: false,
        checked_amount: min_amount_out.clone(),
        swaps: vec![swap],
        native_action: None,
    };

    // let encoded_solutions = encoder.encode_solutions(vec![solution])?;
    // let encoded_solutions = encoder.encode_full_calldata(vec![solution])?;
    // let encoded = &encoded_solutions[0];

    // let executor_address = Address::from_slice(&encoded.interacting_with);
    // let approve_function_signature = "approve(address,uint256)";
    // let args = (executor_address, biguint_to_u256(&amount_in));
    // let approve_calldata = encode_input(approve_function_signature, args.abi_encode());
    //
    // let encoded_data = create_multitrade_calldata(
    //     Address::from_slice(sell_token.address.as_ref()),
    //     executor_address,
    //     approve_calldata,
    //     encoded.swaps.clone(),
    // )?;
    //
    // info!("Encoded data: 0x{}", hex::encode(&encoded_data));
    //
    // Ok(encoded_data)
    let transactions = encoder.encode_full_calldata(vec![solution])?;
    let transaction = &transactions[0];

    info!("=== Transaction Debug ===");
    info!("To: 0x{}", hex::encode(&transaction.to));
    info!("Data length: {} bytes", transaction.data.len());
    info!(
        "Function selector: 0x{}",
        hex::encode(&transaction.data[..4])
    );
    info!("========================");

    let router_address = Address::from_slice(&transaction.to);

    // ✅ Используй transaction.data вместо encoded.swaps
    let swap_calldata = transaction.data.clone();

    let amount_u256 = biguint_to_u256(&amount_in);

    let approve_function_signature = "approve(address,uint256)";
    let args = (router_address, amount_u256);
    let approve_calldata = encode_input(approve_function_signature, args.abi_encode());

    let encoded_data = create_multitrade_calldata(
        Address::from_slice(sell_token.address.as_ref()),
        router_address,
        approve_calldata,
        swap_calldata, // ✅ Теперь это полный calldata
    )?;

    info!("Final calldata: 0x{}", hex::encode(&encoded_data));

    Ok(encoded_data)
}
