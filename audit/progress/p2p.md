# Progress: p2p

Status: complete
Entry points analyzed: 
- crates/sui-network/src/state_sync/mod.rs:814 handle_message
- crates/sui-network/src/discovery/mod.rs:418 handle_message  
- crates/sui-network/src/randomness/mod.rs:262 handle_message

Entry points remaining: none (all 3 analyzed)

Findings written:
- p2p-P9-01: Unbounded Peer Address Storage (discovery)
- p2p-P9-02: Unbounded Future Epoch Partial Signatures Storage (randomness)

Notes:

## Analysis Summary

### Discovery Module (DiscoveryMessage::handle_message at mod.rs:418)
- Handles: PeerAddressChange, ReceivedNodeInfo, ConfiguredPeersUpdated, PeerFailureReport
- Main vulnerability: peer_addresses HashMap has no size cap
- Entry is via endpoint_manager.update_endpoint → DiscoveryMessage::PeerAddressChange
- Validates MAX_ADDRESS_LENGTH, MAX_ADDRESSES_PER_PEER but no global peer count limit

### Randomness Module (RandomnessMessage::handle_message at mod.rs:262)
- Handles: UpdateEpoch, SendPartialSignatures, CompleteRound, ReceiveSignatures
- Main vulnerability: future_epoch_partial_sigs BTreeMap allows unlimited peer submissions per epoch/round
- Entry is via RandomnessMessage::ReceiveSignatures from other network peers
- Limited rounds ahead but NOT limited unique peer submissions per round

### State Sync Module (StateSyncMessage::handle_message at mod.rs:814)
- Handles: StartSyncJob, VerifiedCheckpoint, SyncedCheckpoint
- Data stored in external store (not unbounded in-memory)
- Has sequence number checks but no unbounded memory accumulation
- Low risk from resource exhaustion perspective

## Patterns Checked

- P9 (Network Resource Exhaustion): CONFIRMED - 2 findings
  - Discovery: unbounded peer_addresses HashMap
  - Randomness: unbounded future_epoch_partial_sigs entries
  
- P10 (Cross-Layer Message Integrity): NOT APPLICABLE
  - No bridge/cross-layer messages in these handlers
  
- P11 (Unbounded Computation): NOT APPLICABLE
  - No computation loops triggered by these handlers
  
- P12 (ZK Circuit): NOT APPLICABLE
  - No ZK circuits in P2P handlers

## Cross-Subsystem Observations

- discovery → state_sync: discovery notifies checkpoint via broadcast channel
- randomness → consensus: DKG output required for validity
- No calls to untrusted external subsystems

## Coverage

- All 3 entry points analyzed
- Pattern families P9, P10, P11, P12 checked
- Remaining vulnerabilities in these modules are lower severity
