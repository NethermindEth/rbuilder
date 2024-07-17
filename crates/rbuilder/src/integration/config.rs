pub const CONFIG: &str = r#"
log_json = false
log_level = "info,rbuilder=debug"
telemetry_port = 6060
telemetry_ip = "0.0.0.0"

relay_secret_key = "5eae315483f028b5cdd5d1090ff0c7618b18737ea9bf3c35047189db22835c48"
coinbase_secret_key = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"

cl_node_url = ["http://localhost:3500"]
jsonrpc_server_port = 8645
jsonrpc_server_ip = "0.0.0.0"
el_node_ipc_path = "/tmp/reth.ipc"
extra_data = "⚡🤖"

dry_run = false
dry_run_validation_url = "http://localhost:8545"

blocks_processor_url = "http://block_processor.internal"
ignore_cancellable_orders = true

sbundle_mergeabe_signers = []
# slot_delta_to_start_submits_ms is usually negative since we start bidding BEFORE the slot start
#slot_delta_to_start_submits_ms = -5000
live_builders = ["mp-ordering"]

[[relays]]
name = "custom"
url = "http://0xac6e77dfe25ecd6110b8e780608cce0dab71fdd5ebea22a16c0205200f2f8e2e3ad3b71d3499c54ad14d6c21b41a37ae@localhost:5555"
priority = 0
use_ssz_for_submit = false
use_gzip_for_submit = false

[[builders]]
name = "mgp-ordering"
algo = "ordering-builder"
discard_txs = true
sorting = "mev-gas-price"
failed_order_retries = 1
drop_failed_orders = true

[[builders]]
name = "mp-ordering"
algo = "ordering-builder"
discard_txs = true
sorting = "max-profit"
failed_order_retries = 1
drop_failed_orders = true
"#;
