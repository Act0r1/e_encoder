mod consts;
mod error;
use std::io::Error;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use alloy::primitives::{Uint, keccak256};
use alloy::providers::ext::AnvilApi;
use alloy::transports::http::reqwest;
use alloy::{hex, sol};
use anyhow::{Result, bail};
#[allow(unused_imports)]
use futures::StreamExt;

use tracing::{debug, error, info, trace};
use tracing_subscriber::EnvFilter;
use tycho_execution::encoding::evm::strategy_encoder::strategy_encoders::SingleSwapStrategyEncoder;
use tycho_execution::encoding::tycho_encoder::TychoEncoder;

use crate::consts::{ARBITRAGE_WALLET_ADDRESS, EULER_SWAP_CONTRACT_ADDRESS, OUR_CONTRACT};
use crate::error::StateErrors::Disconnect;
#[allow(unused_imports)]
use alloy::{
    eips::BlockNumberOrTag,
    network::{Ethereum, EthereumWallet},
    node_bindings::Anvil,
    primitives::{Address, B256, Bytes as AlloyBytes, Keccak256, Signature, TxKind, U256},
    providers::{
        Identity, Provider, ProviderBuilder, RootProvider,
        fillers::{FillProvider, JoinFill, WalletFiller},
    },
    rpc::types::{
        TransactionInput, TransactionRequest,
        simulate::{SimBlock, SimulatePayload},
    },
    signers::{SignerSync, local::PrivateKeySigner},
    sol_types::{SolCall, SolStruct, SolValue, eip712_domain},
};
use num_bigint::BigUint;
use tycho_execution::encoding::models::{EncodedSolution, Solution, Swap};
use tycho_execution::encoding::{
    evm::{
        approvals::permit2::PermitSingle,
        encoder_builders::{TychoExecutorEncoderBuilder, TychoRouterEncoderBuilder},
    },
    models::{SwapBuilder, UserTransferType},
    strategy_encoder::StrategyEncoder,
};

#[allow(unused_imports)]
use tycho_simulation::{
    evm::{
        protocol::{
            filters::uniswap_v4_euler_hook_pool_filter, u256_num::biguint_to_u256,
            uniswap_v4::state::UniswapV4State,
        },
        stream::ProtocolStreamBuilder,
    },
    tycho_client::feed::component_tracker::ComponentFilter,
    tycho_common::hex_bytes::Bytes,
    tycho_common::models::Chain,
    tycho_common::{models::token::Token, simulation::protocol_sim::ProtocolSim},
    utils::load_all_tokens,
};

#[allow(non_snake_case)]
#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(true)
        .with_line_number(true)
        .init();

    info!("🚀 Starting EulerSwap application");

    dotenv::dotenv().ok();

    let rpc_url = std::env::var("RPC_URL").expect("Not found RPC_URL, pls add it to .env");
    let TYCHO_API_KEY = std::env::var("TYCHO_API_KEY").expect("TYCHO_API_KEY not set in .env");
    let PRIVATE_KEY = std::env::var("PRIVATE_KEY").expect("PRIVATE_KEY  not set in .env");

    info!("📡 Loading all tokens from Tycho API");
    let all_tokens = load_all_tokens(
        "tycho-beta.propellerheads.xyz",
        false,
        Some(&TYCHO_API_KEY),
        false,
        Chain::Ethereum,
        None,
        None,
    )
    .await
    .map_err(Disconnect);
    info!("Loaded all tokens, starting protocl stream...");

    let tokens = match all_tokens {
        Ok(tokens) => {
            info!(token_count = tokens.len(), "✅ Successfully loaded tokens",);
            debug!(
                "Token addresses: {:?}",
                tokens.keys().take(5).collect::<Vec<_>>()
            );
            tokens
        }
        Err(Disconnect(sim_error)) => {
            error!(error = %sim_error, "❌ Failed to load tokens from Tycho");
            bail!("Details: {}", sim_error);
        }
    };

    let tvl_filter = ComponentFilter::with_tvl_range(100.0, 100.0);

    info!("🔧 Building protocol stream with exchanges");
    let protocol_stream =
        ProtocolStreamBuilder::new("tycho-beta.propellerheads.xyz", Chain::Ethereum)
            .exchange::<UniswapV4State>("uniswap_v4", tvl_filter.clone(), None)
            // .exchange::<UniswapV4State>("uniswap_v4_hooks", tvl_filter.clone(), None)
            .auth_key(Some(TYCHO_API_KEY.to_string()))
            .disable_compression() // Disable compression to avoid page_size > 500 error
            .skip_state_decode_failures(true)
            .set_tokens(tokens)
            .await
            .build()
            .await;
    let mut stream = match protocol_stream {
        Ok(strs) => strs,
        Err(e) => {
            bail!("Failed to build ProtocolStreamBuilder: {:?}", e);
        }
    };

    // EulerSwap

    info!("✅ Protocol stream built successfully, starting message loop");

    while let Some(msg) = stream.next().await {
        trace!(message = ?msg, "Full message details");

        match msg {
            Ok(m) => {
                let pairs = m.new_pairs;

                for (id, states) in m.states.iter() {
                    if let Some(component) = pairs.get(id) {
                        let addrs = &component.tokens;
                        let amount_in =
                            BigUint::from((1f64 * 10f64.powi(addrs[0].decimals as i32)) as u128);
                        let provider = ProviderBuilder::new().connect_http(
                            reqwest::Url::from_str(&rpc_url)
                                .expect("Failed build provider with RPC_URL"),
                        );
                        let sell_token = &addrs[0];
                        let buy_token = &addrs[1];
                        info!(
                            "Selling/buying token symbol: {:?}/{:?}",
                            sell_token.symbol, buy_token.symbol
                        );

                        if let Ok(amount_out_result) =
                            states.get_amount_out(amount_in.clone(), sell_token, buy_token)
                            && sell_token.symbol == "WBTC"
                        {
                            info!("Get balance for {:?}", sell_token.symbol);

                            let encoder = TychoExecutorEncoderBuilder::new()
                                .chain(Chain::Ethereum)
                                .build()
                                .expect("Error when creating enocder");

                            let amount_out = amount_out_result.amount.clone();

                            let pk = B256::from_str(PRIVATE_KEY.as_ref())
                                .expect("Failed to convert swapper pk to B256");
                            let signer = PrivateKeySigner::from_bytes(&pk)
                                .expect("Failed to create PrivateKeySigner");
                            let deadline = U256::from(
                                SystemTime::now()
                                    .duration_since(UNIX_EPOCH)
                                    .unwrap()
                                    .as_secs()
                                    + 300,
                            );
                            let component_ = component.clone().into();
                            let swap = Swap {
                                component: component_,
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
                                checked_amount: amount_out.clone(),
                                swaps: vec![swap],
                                native_action: None,
                            };
                            let encoded_solutions = encoder
                                .encode_solutions(vec![solution.clone()])
                                .expect("Failed to encode solution");

                            let encoded = &encoded_solutions[0];
                            let executor_address = Address::from_slice(&encoded.interacting_with);
                            let approve_function_signature = "approve(address,uint256)";
                            let args = (executor_address, biguint_to_u256(&amount_in));
                            let approve_calldata =
                                encode_input(approve_function_signature, args.abi_encode());
                            let encoded_data = create_multitrade_calldata(
                                Address::from_slice(sell_token.address.as_ref()),
                                executor_address,
                                approve_calldata,
                                encoded.swaps.clone(),
                            ).unwrap();
                            info!("Finish encoded data: 0x{:?}", hex::encode(encoded_data));
                            break;
                        }
                    }
                }
            }
            Err(StreamDecodeError) => {
                error!("Get error: {:?}", StreamDecodeError)
            }
        }
    }

    Ok(())
}

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

fn create_multitrade_calldata(
    token_address: Address,
    executor_address: Address,
    approve_calldata: Vec<u8>,
    swap_calldata: Vec<u8>,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    // Define Data struct
    sol!(
        struct Data {
            address target;
            uint256 value;
            bytes callData;
        }
       #[sol(rpc)]
       function executeInteractions(Data[] interactions, address tokenAddress, uint8 isTest) external payable;

    );

    // Build interactions array
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

    // Encode function call
    let function_sig = "executeInteractions((address,uint256,bytes)[],address,uint8)";
    let function_selector = &keccak256(function_sig.as_bytes())[..4];

    let args = executeInteractionsCall::new((interactions, token_address, 1u8));
    let encoded_args = args.abi_encode();

    Ok(encoded_args)
}
