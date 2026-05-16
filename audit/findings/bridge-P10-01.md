# bridge-P10-01: Bridge Signatures for Reverted Transactions

| Field | Value |
|-------|-------|
| **Severity** | High |
| **Confidence** | 75 |
| **Pattern** | P10 - Cross-Layer / Bridge Message Integrity Failures |
| **Location** | `crates/sui-bridge/src/server/handler.rs:128-136` and `crates/sui-bridge/src/server/handler.rs:138-148` |
| **Entry Point** | HTTP endpoints: `/sign/bridge_tx/eth/sui/{tx_hash}/{event_index}` and `/sign/bridge_tx/sui/eth/{tx_digest}/{event_index}` |
| **Impact** | Attacker can obtain bridge authority signatures for transfer messages that never actually executed on the source chain, potentially enabling replay or double-spend attacks if the signed action is later accepted by the bridge executor. |

## Description

The bridge request handlers (handle_eth_tx_hash and handle_sui_tx_digest) retrieve events from external chain transactions and generate signed BridgeAction objects without verifying that the source transaction actually succeeded.

For Ethereum transactions (handle_eth_tx_hash):
- The handler calls eth_client.get_finalized_bridge_action_maybe(tx_hash, event_idx) 
- Located at crates/sui-bridge/src/eth_client.rs:92-136
- The code checks the transaction is finalized but does NOT check receipt.status() to verify the transaction succeeded

For Sui transactions (handle_sui_tx_digest):
- The handler calls sui_client.get_bridge_action_by_tx_digest_and_event_idx_maybe(tx_digest, event_idx)
- Located at crates/sui-bridge/src/sui_client.rs:133-152
- The code queries events by transaction digest but does NOT check the transaction effects/status to verify the transaction succeeded

## Trigger Scenario

1. Attacker initiates a token transfer on Ethereum that is designed to revert (e.g., gas limit reached, contract logic failure)
2. The transaction emits a valid bridge event before reverting
3. Attacker calls the bridge server endpoint /sign/bridge_tx/eth/sui/{tx_hash}/{event_index} with the reverted transaction's hash
4. Bridge server retrieves the event from the reverted transaction, generates a signed BridgeAction
5. Attacker obtains a valid signature for an action that never actually executed

The same attack applies to Sui transactions via /sign/bridge_tx/sui/eth/{tx_digest}/{event_index}.

## Quantitative Assessment

- Cost per attack: Minimal - just an HTTP request to the bridge server
- Rate limit: None observed at the handler level (only global request size limits at mod.rs:41-42)
- Impact: Any successfully reverted transaction with a bridge event can be signed by the bridge authority, creating a valid-looking SignedBridgeAction for a non-existent transfer

## Existing Mitigations

- Transaction finalization check (Eth): Code at eth_client.rs:111-113 checks receipt_block_num > last_finalized_block_id and returns TxNotFinalized - this prevents reorg attacks but NOT reverted transactions
- Contract address validation (Eth): Code at eth_client.rs:121-123 validates the event came from a recognized bridge contract - this is present but only validates source, not transaction success
- Package validation (Sui): Code at sui_client.rs:143-144 validates event came from the bridge package - this is present but only validates source, not transaction success
- Error type exists but unused: BridgeError::OriginTxFailed exists in error.rs:10-11 but is never returned by either handler

## Missing Defenses

- Transaction success verification (Eth): Check receipt.status() to ensure the transaction succeeded before processing the event
- Transaction success verification (Sui): Query transaction effects and check status.success() before processing the event
- Use existing error type: The OriginTxFailed error exists but is not used - it should be returned when a reverted transaction is detected

## Recommendation

1. In eth_client.rs:get_finalized_bridge_action_maybe(), add after line 107:
   if !receipt.status().is_ok() {
       return Err(BridgeError::OriginTxFailed);
   }

2. In sui_client.rs:get_bridge_action_by_tx_digest_and_event_idx_maybe(), add before processing events: query transaction effects and verify success before extracting events.

3. Ensure OriginTxFailed returns appropriate HTTP status (likely 400 Bad Request or 409 Conflict)
