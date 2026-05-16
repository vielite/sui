# Progress: bridge

Status: complete
Entry points analyzed: 
- crates/sui-bridge/src/server/handler.rs:24 handle_eth_tx_hash
- crates/sui-bridge/src/server/handler.rs:32 handle_sui_tx_digest
- crates/sui-bridge/src/server/handler.rs:40 handle_sui_token_transfer
- crates/sui-bridge/src/server/handler.rs:48 handle_governance_action
- crates/sui-bridge/src/server/mod.rs:274 handle_eth_tx_hash (HTTP endpoint wrapper)
- crates/sui-bridge/src/server/mod.rs:326 handle_sui_token_transfer (HTTP endpoint wrapper)
- crates/sui-bridge/src/monitor.rs:107 handle_sui_events

Entry points remaining: 
- None - all specified entry points have been analyzed

Findings written: 
- bridge-P10-01: Bridge Signatures for Reverted Transactions (High, 75 confidence)

Notes: 
- Cross-subsystem call: handler.rs calls eth_client (external Ethereum RPC) and sui_client (Sui fullnode) to verify bridge events - these clients query external providers
- Cross-subsystem call: monitor.rs updates bridge_auth_agg which is shared state - potential race condition if multiple events update simultaneously
- Pattern P10 matched: The reverted transaction signing vulnerability is exactly the "Reverted-transaction log emission" pattern described in client-attack-patterns-3.md
- Additional observation: GovernanceVerifier has a TODO at governance_verifier.rs:29 for nonce validation - currently allows signing of old governance actions with stale nonces (not a confirmed vulnerability as it may be validated elsewhere in the pipeline)
- Additional observation: handle_sui_token_transfer uses handle_governance_action internally, so governance action path overlaps with token transfer path
- Defenses present: Request size limits (8KB URI, 64KB body), list size validation (255 max items), finalization check (Eth only), package/contract address validation
