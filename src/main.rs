mod config;
mod consts;
mod encoding;
mod error;
mod stream_handler;


use alloy::providers::{Provider, ProviderBuilder};
use anyhow::{Result, bail};
use futures::StreamExt;
use num_bigint::BigUint;
use tracing::{error, info, trace};
use tracing_subscriber::EnvFilter;

use tycho_execution::encoding::evm::encoder_builders::TychoRouterEncoderBuilder;
use tycho_simulation::evm::protocol::filters::uniswap_v4_euler_hook_pool_filter;
use tycho_simulation::evm::protocol::uniswap_v4::state::UniswapV4State;
use tycho_simulation::evm::stream::ProtocolStreamBuilder;
use tycho_simulation::tycho_client::feed::component_tracker::ComponentFilter;
use tycho_simulation::tycho_common::models::Chain;
use tycho_simulation::utils::load_all_tokens;

use crate::config::AppConfig;
use crate::error::StateErrors::Disconnect;
use crate::stream_handler::process_swap;

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

    let config = AppConfig::from_env()?;

    info!("📡 Loading all tokens from Tycho API");
    let all_tokens = load_all_tokens(
        "tycho-beta.propellerheads.xyz",
        false,
        Some(&config.tycho_api_key),
        false,
        Chain::Ethereum,
        None,
        None,
    )
    .await
    .map_err(Disconnect);

    let tokens = match all_tokens {
        Ok(tokens) => {
            info!(token_count = tokens.len(), "✅ Successfully loaded tokens");
            trace!(
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
            // .exchange::<UniswapV4State>("uniswap_v4_hooks", tvl_filter.clone(), Some(uniswap_v4_euler_hook_pool_filter))

            .auth_key(Some(config.tycho_api_key.clone()))
            .disable_compression()
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
    let encoder = TychoRouterEncoderBuilder::new()
        .user_transfer_type(tycho_execution::encoding::models::UserTransferType::TransferFrom)
        .chain(Chain::Ethereum)
        .build()?;


    let provider = ProviderBuilder::new().connect_http(config.rpc_url);


    info!("✅ Protocol stream built successfully, starting message loop");

    while let Some(msg) = stream.next().await {
        trace!(message = ?msg, "Full message details");

        match msg {
            Ok(m) => {
                let pairs = m.new_pairs;

                for (id, states) in m.states.iter() {
                    if let Some(component) = pairs.get(id) {
                        let addrs = &component.tokens;
                        let sell_token = &addrs[0];
                        let buy_token = &addrs[1];

                        let amount_in = BigUint::from(1000u128); // TODO: костыль, нужно что сумма определялась по другому

                        info!(
                            "Selling/buying token symbol: {}/{}",
                            sell_token.symbol, buy_token.symbol
                        );

                        if let Ok(amount_out_result) =
                            states.get_amount_out(amount_in.clone(), sell_token, buy_token)
                            && sell_token.symbol == "WBTC"
                        {
                            let amount_out = amount_out_result.amount.clone();
                            info!("Processing swap for {}", sell_token.symbol);
                            info!("Amount: {}", amount_out);

                            match process_swap(
                                component,
                                sell_token,
                                buy_token,
                                amount_in,
                                amount_out,
                                &config.private_key,
                                encoder.as_ref(),
                            ) {
                                Ok(tx_request) => {
                                    match provider.estimate_gas(tx_request).await {
                                        Ok(gas) => {
                                            info!("Estimated gas: {}", gas);
                                        }
                                        Err(e) => {
                                            error!("❌ Failed to estimate gas: {}", e);
                                        }
                                    }
                                }
                                Err(e) => {
                                    error!("❌ Failed to process swap: {}", e);
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                error!("❌ Stream error: {:?}", e);
            }
        }
    }

    Ok(())
}
