/******************************************************************************
 * Copyright © 2014-2018 The SuperNET Developers.                             *
 *                                                                            *
 * See the AUTHORS, DEVELOPER-AGREEMENT and LICENSE files at                  *
 * the top-level directory of this distribution for the individual copyright  *
 * holder information and the developer policies on copyright and licensing.  *
 *                                                                            *
 * Unless otherwise agreed in a custom licensing agreement, no part of the    *
 * SuperNET software, including this file may be copied, modified, propagated *
 * or distributed except according to the terms contained in the LICENSE file *
 *                                                                            *
 * Removal or modification of this copyright notice is prohibited.            *
 ******************************************************************************/
//
//  rpc.rs
//
//  Copyright © 2014-2018 SuperNET. All rights reserved.
//

use coins::{convert_address, convert_utxo_address, get_enabled_coins, get_trade_fee, kmd_rewards_info, my_tx_history,
            send_raw_transaction, set_required_confirmations, set_requires_notarization, show_priv_key,
            validate_address, withdraw};
#[cfg(not(target_arch = "wasm32"))] use common::log::warn;
use common::log::{error, info};
use common::mm_ctx::MmArc;
#[cfg(not(target_arch = "wasm32"))]
use common::wio::{CORE, CPUPOOL};
use common::{err_to_rpc_json_string, err_tp_rpc_json, HyRes};
use futures::compat::Future01CompatExt;
use futures::future::{join_all, FutureExt, TryFutureExt};
use http::header::{HeaderValue, ACCESS_CONTROL_ALLOW_ORIGIN};
use http::request::Parts;
use http::{Method, Request, Response};
#[cfg(not(target_arch = "wasm32"))]
use hyper::{self, Body, Server};
use serde_json::{self as json, Value as Json};
use std::future::Future as Future03;
use std::net::SocketAddr;

use crate::mm2::lp_ordermatch::{best_orders_rpc, buy, cancel_all_orders, cancel_order, my_orders, order_status,
                                orderbook_depth_rpc, orderbook_rpc, sell, set_price};
use crate::mm2::lp_swap::{active_swaps_rpc, all_swaps_uuids_by_filter, ban_pubkey_rpc, coins_needed_for_kick_start,
                          import_swaps, list_banned_pubkeys_rpc, max_taker_vol, my_recent_swaps, my_swap_status,
                          recover_funds_of_swap, stats_swap_status, trade_preimage, unban_pubkeys_rpc};

use self::lp_commands::*;
#[path = "rpc/lp_commands.rs"] pub mod lp_commands;

/// Lists the RPC method not requiring the "userpass" authentication.  
/// None is also public to skip auth and display proper error in case of method is missing
const PUBLIC_METHODS: &[Option<&str>] = &[
    // Sorted alphanumerically (on the first letter) for readability.
    Some("fundvalue"),
    Some("getprice"),
    Some("getpeers"),
    Some("getcoins"),
    Some("help"),
    Some("metrics"),
    Some("notify"), // Manually checks the peer's public key.
    Some("orderbook"),
    Some("passphrase"), // Manually checks the "passphrase".
    Some("pricearray"),
    Some("psock"),
    Some("statsdisp"),
    Some("stats_swap_status"),
    Some("tradesarray"),
    Some("ticker"),
    None,
];

#[allow(unused_macros)]
macro_rules! unwrap_or_err_response {
    ($e:expr, $($args:tt)*) => {
        match $e {
            Ok(ok) => ok,
            Err(err) => return rpc_err_response(500, &ERRL!("{}", err)),
        }
    };
}

fn auth(json: &Json, ctx: &MmArc) -> Result<(), &'static str> {
    if !PUBLIC_METHODS.contains(&json["method"].as_str()) {
        if !json["userpass"].is_string() {
            return Err("Userpass is not set!");
        }

        if json["userpass"] != ctx.conf["rpc_password"] {
            return Err("Userpass is invalid!");
        }
    }
    Ok(())
}

/// Result of `fn dispatcher`.
pub enum DispatcherRes {
    /// `fn dispatcher` has found a Rust handler for the RPC "method".
    Match(HyRes),
    /// No handler found by `fn dispatcher`. Returning the `Json` request in order for it to be handled elsewhere.
    NoMatch(Json),
}

/// Using async/await (futures 0.3) in `dispatcher`
/// will pave the way for porting the remaining system threading code to async/await green threads.
fn hyres(handler: impl Future03<Output = Result<Response<Vec<u8>>, String>> + Send + 'static) -> HyRes {
    Box::new(handler.boxed().compat())
}

/// The dispatcher, with full control over the HTTP result and the way we run the `Future` producing it.
///
/// Invoked both directly from the HTTP endpoint handler below and in a delayed fashion from `lp_command_q_loop`.
///
/// Returns `None` if the requested "method" wasn't found among the ported RPC methods and has to be handled elsewhere.
pub fn dispatcher(req: Json, ctx: MmArc) -> DispatcherRes {
    let method = match req["method"].clone() {
        Json::String(method) => method,
        _ => return DispatcherRes::NoMatch(req),
    };
    DispatcherRes::Match(match &method[..] {
        // Sorted alphanumerically (on the first latter) for readability.
        // "autoprice" => lp_autoprice (ctx, req),
        "active_swaps" => hyres(active_swaps_rpc(ctx, req)),
        "all_swaps_uuids_by_filter" => all_swaps_uuids_by_filter(ctx, req),
        "ban_pubkey" => hyres(ban_pubkey_rpc(ctx, req)),
        "best_orders" => hyres(best_orders_rpc(ctx, req)),
        "buy" => hyres(buy(ctx, req)),
        "cancel_all_orders" => hyres(cancel_all_orders(ctx, req)),
        "cancel_order" => hyres(cancel_order(ctx, req)),
        "coins_needed_for_kick_start" => hyres(coins_needed_for_kick_start(ctx)),
        "convertaddress" => hyres(convert_address(ctx, req)),
        "convert_utxo_address" => hyres(convert_utxo_address(ctx, req)),
        "disable_coin" => hyres(disable_coin(ctx, req)),
        "electrum" => hyres(electrum(ctx, req)),
        "enable" => hyres(enable(ctx, req)),
        "get_enabled_coins" => hyres(get_enabled_coins(ctx)),
        "get_gossip_mesh" => hyres(get_gossip_mesh(ctx)),
        "get_gossip_peer_topics" => hyres(get_gossip_peer_topics(ctx)),
        "get_gossip_topic_peers" => hyres(get_gossip_topic_peers(ctx)),
        "get_my_peer_id" => hyres(get_my_peer_id(ctx)),
        "get_peers_info" => hyres(get_peers_info(ctx)),
        "get_relay_mesh" => hyres(get_relay_mesh(ctx)),
        "get_trade_fee" => hyres(get_trade_fee(ctx, req)),
        // "fundvalue" => lp_fundvalue (ctx, req, false),
        "help" => help(),
        "import_swaps" => {
            #[cfg(not(target_arch = "wasm32"))]
            {
                Box::new(CPUPOOL.spawn_fn(move || hyres(import_swaps(ctx, req))))
            }
            #[cfg(target_arch = "wasm32")]
            {
                return DispatcherRes::NoMatch(req);
            }
        },
        "kmd_rewards_info" => hyres(kmd_rewards_info(ctx)),
        // "inventory" => inventory (ctx, req),
        "list_banned_pubkeys" => hyres(list_banned_pubkeys_rpc(ctx)),
        "max_taker_vol" => hyres(max_taker_vol(ctx, req)),
        "metrics" => metrics(ctx),
        "min_trading_vol" => hyres(min_trading_vol(ctx, req)),
        "my_balance" => hyres(my_balance(ctx, req)),
        "my_orders" => hyres(my_orders(ctx)),
        "my_recent_swaps" => my_recent_swaps(ctx, req),
        "my_swap_status" => my_swap_status(ctx, req),
        "my_tx_history" => my_tx_history(ctx, req),
        "order_status" => hyres(order_status(ctx, req)),
        "orderbook" => hyres(orderbook_rpc(ctx, req)),
        "orderbook_depth" => hyres(orderbook_depth_rpc(ctx, req)),
        "sim_panic" => hyres(sim_panic(req)),
        "recover_funds_of_swap" => {
            #[cfg(not(target_arch = "wasm32"))]
            {
                Box::new(CPUPOOL.spawn_fn(move || hyres(recover_funds_of_swap(ctx, req))))
            }
            #[cfg(target_arch = "wasm32")]
            {
                return DispatcherRes::NoMatch(req);
            }
        },
        "sell" => hyres(sell(ctx, req)),
        "show_priv_key" => hyres(show_priv_key(ctx, req)),
        "send_raw_transaction" => hyres(send_raw_transaction(ctx, req)),
        "set_required_confirmations" => hyres(set_required_confirmations(ctx, req)),
        "set_requires_notarization" => hyres(set_requires_notarization(ctx, req)),
        "setprice" => hyres(set_price(ctx, req)),
        "stats_swap_status" => stats_swap_status(ctx, req),
        "stop" => stop(ctx),
        "trade_preimage" => hyres(trade_preimage(ctx, req)),
        "unban_pubkeys" => hyres(unban_pubkeys_rpc(ctx, req)),
        "validateaddress" => hyres(validate_address(ctx, req)),
        "version" => version(),
        "withdraw" => hyres(withdraw(ctx, req)),
        _ => return DispatcherRes::NoMatch(req),
    })
}

async fn process_json_batch_requests(ctx: MmArc, requests: &[Json], client: SocketAddr) -> Result<Json, String> {
    let mut futures = Vec::with_capacity(requests.len());
    for request in requests {
        futures.push(process_single_request(ctx.clone(), request.clone(), client));
    }
    let results = join_all(futures).await;
    let responses: Vec<_> = results
        .into_iter()
        .map(|resp| match resp {
            Ok(r) => match json::from_slice(r.body()) {
                Ok(j) => j,
                Err(e) => {
                    error!("Response {:?} is not a valid JSON, error: {}", r, e);
                    Json::Null
                },
            },
            Err(e) => err_tp_rpc_json(e),
        })
        .collect();
    Ok(Json::Array(responses))
}

#[cfg(target_arch = "wasm32")]
async fn process_json_request(ctx: MmArc, req_json: Json, client: SocketAddr) -> Result<Json, String> {
    if let Some(requests) = req_json.as_array() {
        return process_json_batch_requests(ctx, &requests, client)
            .await
            .map_err(|e| ERRL!("{}", e));
    }

    let r = try_s!(process_single_request(ctx, req_json, client).await);
    json::from_slice(r.body()).map_err(|e| ERRL!("Response {:?} is not a valid JSON, error: {}", r, e))
}

#[cfg(not(target_arch = "wasm32"))]
async fn process_json_request(ctx: MmArc, req_json: Json, client: SocketAddr) -> Result<Response<Vec<u8>>, String> {
    if let Some(requests) = req_json.as_array() {
        let response = try_s!(process_json_batch_requests(ctx, &requests, client).await);
        let res = try_s!(json::to_vec(&response));
        return Ok(try_s!(Response::builder().body(res)));
    }

    process_single_request(ctx, req_json, client).await
}

async fn process_single_request(ctx: MmArc, req: Json, client: SocketAddr) -> Result<Response<Vec<u8>>, String> {
    // https://github.com/artemii235/SuperNET/issues/368
    let local_only = ctx.conf["rpc_local_only"].as_bool().unwrap_or(true);
    if local_only && !client.ip().is_loopback() && !PUBLIC_METHODS.contains(&req["method"].as_str()) {
        return ERR!("Selected method can be called from localhost only!");
    }
    try_s!(auth(&req, &ctx));

    let handler = match dispatcher(req, ctx.clone()) {
        DispatcherRes::Match(handler) => handler,
        DispatcherRes::NoMatch(req) => return ERR!("No such method: {:?}", req["method"]),
    };
    let res = try_s!(handler.compat().await);
    Ok(res)
}

#[cfg(not(target_arch = "wasm32"))]
async fn rpc_service(req: Request<Body>, ctx_h: u32, client: SocketAddr) -> Response<Body> {
    /// Unwraps a result or propagates its error 500 response with the specified headers (if they are present).
    macro_rules! try_sf {
        ($value: expr $(, $header_key:expr => $header_val:expr)*) => {
            match $value {
                Ok(ok) => ok,
                Err(err) => {
                    error!("RPC error response: {}", err);
                    let ebody = err_to_rpc_json_string(&fomat!((err)));
                    // generate a `Response` with the headers specified in `$header_key` and `$header_val`
                    let response = Response::builder().status(500) $(.header($header_key, $header_val))* .body(Body::from(ebody)).unwrap();
                    return response;
                },
            }
        };
    }

    async fn process_rpc_request(
        ctx: MmArc,
        req: Parts,
        req_json: Json,
        client: SocketAddr,
    ) -> Result<Response<Vec<u8>>, String> {
        if req.method != Method::POST {
            return ERR!("Only POST requests are supported!");
        }

        process_json_request(ctx, req_json, client).await
    }

    let ctx = try_sf!(MmArc::from_ffi_handle(ctx_h));
    // https://github.com/artemii235/SuperNET/issues/219
    let rpc_cors = match ctx.conf["rpccors"].as_str() {
        Some(s) => try_sf!(HeaderValue::from_str(s)),
        None => HeaderValue::from_static("http://localhost:3000"),
    };

    // Convert the native Hyper stream into a portable stream of `Bytes`.
    let (req, req_body) = req.into_parts();
    let req_bytes = try_sf!(hyper::body::to_bytes(req_body).await, ACCESS_CONTROL_ALLOW_ORIGIN => rpc_cors);
    let req_json: Json = try_sf!(json::from_slice(&req_bytes), ACCESS_CONTROL_ALLOW_ORIGIN => rpc_cors);

    let res = try_sf!(process_rpc_request(ctx, req, req_json, client).await, ACCESS_CONTROL_ALLOW_ORIGIN => rpc_cors);
    let (mut parts, body) = res.into_parts();
    parts.headers.insert(ACCESS_CONTROL_ALLOW_ORIGIN, rpc_cors);
    Response::from_parts(parts, Body::from(body))
}

#[cfg(not(target_arch = "wasm32"))]
pub extern "C" fn spawn_rpc(ctx_h: u32) {
    use hyper::server::conn::AddrStream;
    use hyper::service::{make_service_fn, service_fn};
    use std::convert::Infallible;

    // NB: We need to manually handle the incoming connections in order to get the remote IP address,
    // cf. https://github.com/hyperium/hyper/issues/1410#issuecomment-419510220.
    // Although if the ability to access the remote IP address is solved by the Hyper in the future
    // then we might want to refactor into starting it ideomatically in order to benefit from a more graceful shutdown,
    // cf. https://github.com/hyperium/hyper/pull/1640.

    let ctx = MmArc::from_ffi_handle(ctx_h).expect("No context");

    let rpc_ip_port = ctx.rpc_ip_port().unwrap();
    CORE.0.enter(|| {
        let server = Server::try_bind(&rpc_ip_port).unwrap_or_else(|_| panic!("Can't bind on {}", rpc_ip_port));
        let make_svc = make_service_fn(move |socket: &AddrStream| {
            let remote_addr = socket.remote_addr();
            async move {
                Ok::<_, Infallible>(service_fn(move |req: Request<Body>| async move {
                    let res = rpc_service(req, ctx_h, remote_addr).await;
                    Ok::<_, Infallible>(res)
                }))
            }
        });

        let (shutdown_tx, shutdown_rx) = futures::channel::oneshot::channel::<()>();
        let mut shutdown_tx = Some(shutdown_tx);
        ctx.on_stop(Box::new(move || {
            if let Some(shutdown_tx) = shutdown_tx.take() {
                info!("on_stop] firing shutdown_tx!");
                if shutdown_tx.send(()).is_err() {
                    warn!("on_stop] shutdown_tx already closed");
                }
                Ok(())
            } else {
                ERR!("on_stop callback called twice!")
            }
        }));

        let server = server
            .http1_half_close(false)
            .serve(make_svc)
            .with_graceful_shutdown(shutdown_rx.then(|_| futures::future::ready(())));

        let server = server.then(|r| {
            if let Err(err) = r {
                error!("{}", err);
            };
            futures::future::ready(())
        });

        let rpc_ip_port = ctx.rpc_ip_port().unwrap();
        CORE.0.spawn({
            info!(
                ">>>>>>>>>> DEX stats {}:{} DEX stats API enabled at unixtime.{}  <<<<<<<<<",
                rpc_ip_port.ip(),
                rpc_ip_port.port(),
                gstuff::now_ms() / 1000
            );
            let _ = ctx.rpc_started.pin(true);
            server
        });
    });
}

#[cfg(target_arch = "wasm32")]
pub fn spawn_rpc(ctx_h: u32) {
    use common::wasm_rpc;
    use futures::StreamExt;
    use std::sync::Mutex;

    let ctx = MmArc::from_ffi_handle(ctx_h).expect("No context");
    if ctx.wasm_rpc.is_some() {
        error!("RPC is initialized already");
        return;
    }

    let client: SocketAddr = "127.0.0.1:1"
        .parse()
        .expect("'127.0.0.1:1' must be valid socket address");

    let (request_tx, mut request_rx) = wasm_rpc::channel();
    let ctx_c = ctx.clone();
    let fut = async move {
        while let Some((request_json, response_tx)) = request_rx.next().await {
            let response = process_json_request(ctx_c.clone(), request_json, client).await;
            if let Err(e) = response_tx.send(response) {
                error!("Response is not processed: {:?}", e);
            }
        }
    };
    common::executor::spawn(fut);

    // even if the [`MmCtx::wasm_rpc`] is initialized already, the spawned future above will be shutdown
    if let Err(e) = ctx.wasm_rpc.pin(request_tx) {
        error!("'MmCtx::wasm_rpc' is initialized already: {}", e);
        return;
    };
    if let Err(e) = ctx.rpc_started.pin(true) {
        error!("'MmCtx::rpc_started' is set already: {}", e);
        return;
    }

    info!(
        ">>>>>>>>>> DEX stats API enabled at unixtime.{}  <<<<<<<<<",
        common::now_ms() / 1000
    );
}
