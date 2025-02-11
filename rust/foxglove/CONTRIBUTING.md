# Foxglove Rust SDK

## Development

### Generate Protobuf schemas

```bash
cargo run --bin foxglove-proto-gen
```

### Run the example server

1. `cargo run -p example-ws-server`
2. Open the Foxglove desktop app
3. From the dashboard, click "Open connection..."
4. Confirm the WebSocket URL and click "Open"
5. Create a Raw Messages panel and enter "topic" in the message path field at the top 
