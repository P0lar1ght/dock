//! `turn/cards/*`：插件挂在一轮回答下面的卡片（Dock 的 [`cordis_spine::TurnCards`]，Rhai
//! `host.card`）。轮次用投影里的 `turnId`，和 `item/user_message` 的 `turnId` 是同一个。
//!
//! - `list { threadId }` → `{ turns: [{ turnId, cards }] }`：有卡片的轮次，先发生的在前。
//! - `action { threadId, cardId, action }`（受信连接）→ `{ toast }`：点了卡片上的按钮。回调
//!   跑的是插件脚本，可能调工具、叫醒 agent，所以只给受信连接。
//! - 推送 `turn/cards/updated { turnId, cards }`：这一轮的卡片变了（发了、改了、摘了、插件停了）。

use serde_json::{json, Value};

use cordis::{plugin, Inject, Plugin};
use cordis_spine::{CardView, TurnCards, TURN_CARDS};

use crate::handle::GatewayHandle;
use crate::methods::{method, register_methods, MethodPolicy, GATEWAY_METHODS};
use crate::protocol::{self, RpcError};
use crate::threads;

pub fn gateway_turn_cards() -> Plugin {
    plugin(
        "gateway.turn-cards",
        Inject::from([GATEWAY_METHODS]),
        |ctx, _: &()| {
            let thread = MethodPolicy {
                opens_thread: true,
                ..MethodPolicy::default()
            };
            register_methods(
                ctx,
                vec![
                    (
                        protocol::TURN_CARDS_LIST,
                        thread,
                        method(|gw, params| async move { list(&gw, params) }),
                    ),
                    (
                        protocol::TURN_CARDS_ACTION,
                        MethodPolicy {
                            trusted_only: true,
                            ..thread
                        },
                        method(|gw, params| async move { action(&gw, params).await }),
                    ),
                ],
            )?;
            Ok(None)
        },
    )
}

/// 一张卡（`list` 的一项、`updated` 的 `cards` 里的一项）。
pub fn card_json(card: &CardView) -> Value {
    json!({
        "id": card.key,
        "plugin": card.plugin,
        "title": card.spec.title,
        "text": card.spec.text,
        "tone": if card.spec.tone.is_empty() { "default" } else { card.spec.tone.as_str() },
        "view": card.spec.view.as_ref().map(|v| v.to_value()),
        "actions": card
            .spec
            .actions
            .iter()
            .map(|(id, label)| json!({ "id": id, "label": label }))
            .collect::<Vec<_>>(),
        "live": card.live,
    })
}

/// 一轮的卡片：推送的载荷。
pub fn cards_json(cards: &[CardView]) -> Vec<Value> {
    cards.iter().map(card_json).collect()
}

fn list(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let page = threads::resolve_param(gateway, &params)?;
    let Some(service) = gateway.ctx().get::<TurnCards>(TURN_CARDS) else {
        return Ok(json!({ "turns": [] }));
    };
    let cards = service.list(&page.identity);
    let mut turns: Vec<Value> = Vec::new();
    let mut ordinals: Vec<usize> = cards.iter().map(|c| c.ordinal).collect();
    ordinals.dedup();
    for ordinal in ordinals {
        let Some(turn_id) = gateway.turn_id_at(&page.identity, ordinal) else {
            continue;
        };
        let these: Vec<CardView> = cards
            .iter()
            .filter(|c| c.ordinal == ordinal)
            .cloned()
            .collect();
        turns.push(json!({ "turnId": turn_id, "cards": cards_json(&these) }));
    }
    Ok(json!({ "turns": turns }))
}

async fn action(gateway: &GatewayHandle, params: Value) -> Result<Value, RpcError> {
    let page = threads::resolve_param(gateway, &params)?;
    let card = text(&params, "cardId")?;
    let action = text(&params, "action")?;
    let service = gateway
        .ctx()
        .get::<TurnCards>(TURN_CARDS)
        .ok_or_else(|| RpcError::app("unavailable", "turnCards is not mounted"))?;
    let toast = service
        .action(&page.identity, card, action)
        .await
        .map_err(|e| RpcError::app("card_failed", e))?;
    Ok(json!({ "toast": toast }))
}

fn text<'a>(params: &'a Value, key: &str) -> Result<&'a str, RpcError> {
    params
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| RpcError::invalid_params(format!("缺少 {key}")))
}
