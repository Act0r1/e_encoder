use std::str::FromStr;

use alloy::hex;
use alloy::primitives::{Address, B256, Bytes as AlloyBytes, U256};
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use alloy::sol_types::SolValue;
use anyhow::{Result, bail};
use num_bigint::BigUint;
use tracing::info;

use tycho_execution::encoding::models::{Solution, Swap};
use tycho_execution::encoding::tycho_encoder::TychoEncoder;
use tycho_simulation::evm::protocol::u256_num::biguint_to_u256;
use tycho_simulation::protocol::models::ProtocolComponent;
use tycho_simulation::tycho_common::hex_bytes::Bytes;
use tycho_simulation::tycho_common::models::token::Token;

use crate::encoding::{create_multitrade_calldata, encode_input};
use crate::consts::{OUR_CONTRACT, ARBITRAGE_WALLET_ADDRESS};


#[allow(clippy::too_many_arguments)]
pub fn process_swap(
    component: &ProtocolComponent,
    sell_token: &Token,
    buy_token: &Token,
    amount_in: BigUint,
    amount_out: BigUint,
    private_key: &str,
    encoder: &dyn TychoEncoder
) -> Result<TransactionRequest> {
    info!(
        "Processing swap: {} -> {}",
        sell_token.symbol, buy_token.symbol
    );

    // let encoder = TychoRouterEncoderBuilder::new()
    //     .user_transfer_type(tycho_execution::encoding::models::UserTransferType::TransferFrom)
    //     .chain(Chain::Ethereum)
    //     .build()?;

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

    let encoded_solutions = encoder.encode_solutions(vec![solution.clone()])?;
    let encoded = &encoded_solutions[0];

    let router_address = Address::from_slice(&encoded.interacting_with);

    // Build the full method calldata like encode_tycho_router_call does
    // encoded.swaps is just the route data, we need all the other params too
    let given_amount = biguint_to_u256(&solution.given_amount);
    let min_amount_out_u256 = biguint_to_u256(&min_amount_out);
    let given_token_addr = Address::from_slice(&solution.given_token);
    let checked_token_addr = Address::from_slice(&solution.checked_token);
    let receiver_addr = Address::from_slice(&solution.receiver);

    // Determine wrap/unwrap (none for normal swaps)
    let wrap = false;
    let unwrap = false;
    // transferFrom = true since we're using TransferFrom user transfer type
    let transfer_from = true;

    // Build method calldata based on function signature
    let method_calldata = if encoded.function_signature.contains("singleSwap") {
        (
            given_amount,
            given_token_addr,
            checked_token_addr,
            min_amount_out_u256,
            wrap,
            unwrap,
            receiver_addr,
            transfer_from,
            AlloyBytes::from(encoded.swaps.clone()),
        )
            .abi_encode()
    } else if encoded.function_signature.contains("sequentialSwap") {
        (
            given_amount,
            given_token_addr,
            checked_token_addr,
            min_amount_out_u256,
            wrap,
            unwrap,
            receiver_addr,
            transfer_from,
            AlloyBytes::from(encoded.swaps.clone()),
        )
            .abi_encode()
    } else if encoded.function_signature.contains("splitSwap") {
        let n_tokens = U256::from(encoded.n_tokens);
        (
            given_amount,
            given_token_addr,
            checked_token_addr,
            min_amount_out_u256,
            wrap,
            unwrap,
            n_tokens,
            receiver_addr,
            transfer_from,
            AlloyBytes::from(encoded.swaps.clone()),
        )
            .abi_encode()
    } else {
        bail!("Unsupported function signature: {}", encoded.function_signature);
    };

    let swap_calldata = encode_input(&encoded.function_signature, method_calldata);

    info!("=== Encoded Solution Debug ===");
    info!("Router: 0x{}", hex::encode(&encoded.interacting_with));
    info!("Function signature: {}", encoded.function_signature);
    info!("Swaps data length: {} bytes", encoded.swaps.len());
    info!("Full swap calldata length: {} bytes", swap_calldata.len());
    info!(
        "Function selector: 0x{}",
        hex::encode(&swap_calldata[..4])
    );
    info!("===============================");
    let amount_u256 = biguint_to_u256(&amount_in);
    let approve_function_signature = "approve(address,uint256)";
    let args = (router_address, amount_u256);
    let approve_calldata = encode_input(approve_function_signature, args.abi_encode());
    let encoded_data = create_multitrade_calldata(
        Address::from_slice(sell_token.address.as_ref()),
        router_address,
        approve_calldata,
        swap_calldata,
    )?;

    info!("Final calldata: 0x{}", hex::encode(&encoded_data));

    let tx_request = TransactionRequest::default()
        .to(OUR_CONTRACT)
        .from(ARBITRAGE_WALLET_ADDRESS)
        .input(AlloyBytes::from(encoded_data).into())
        .value(U256::ZERO);

    Ok(tx_request)
}
