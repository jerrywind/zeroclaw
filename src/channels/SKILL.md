---
name: create_channel_adapter
description: Instructions for creating a new channel adapter for messaging platforms (e.g., Discord, Slack, etc.)
---

# Create Channel Adapter

This skill guides you through adding a new messaging channel adapter to ZeroClaw. Channel adapters allow the bot to communicate across different platforms like Telegram, Discord, Slack, etc.

## Prerequisites

1.  **Understand the Architecture**:
    *   Channels implement the `Channel` trait (`src/channels/traits.rs`).
    *   They are managed in `src/channels/mod.rs`.
    *   Configuration is handled in `src/config` (specifically `schema.rs`).

2.  **External Requirements**:
    *   API documentation for the target platform.
    *   API keys/tokens for testing.
    *   Rust crate for the platform (optional but recommended, e.g., `serenity` for Discord, `teloxide` or `reqwest` for Telegram).

## Steps to Add a New Channel

### 1. Create the Channel Module

Create a new file in `src/channels/` (e.g., `src/channels/myplatform.rs`).

Implement the `Channel` trait:

```rust
use async_trait::async_trait;
use super::traits::{Channel, ChannelMessage};
use anyhow::Result;

pub struct MyPlatformChannel {
    // fields like api_token, client, etc.
}

impl MyPlatformChannel {
    pub fn new(/* config params */) -> Self {
        Self { /* ... */ }
    }
}

#[async_trait]
impl Channel for MyPlatformChannel {
    fn name(&self) -> &str {
        "myplatform"
    }

    async fn send(&self, message: &str, recipient: &str) -> Result<()> {
        // Implement sending logic
        Ok(())
    }

    async fn listen(&self, tx: tokio::sync::mpsc::Sender<ChannelMessage>) -> Result<()> {
        // Implement long-polling or websocket listening
        // Convert platform messages to ChannelMessage and send via tx
        Ok(())
    }
    
    // Optional overrides
    async fn health_check(&self) -> bool {
        // Pinging the API
        true
    }
}
```

### 2. Update Configuration

1.  Open `src/config/schema.rs`.
2.  Add a configuration struct for your channel.
3.  Add the struct to `ChannelsConfig`.

```rust
// In src/config/schema.rs

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MyPlatformConfig {
    pub api_key: String,
    pub allowed_users: Vec<String>,
    // other specific config
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct ChannelsConfig {
    // ... existing channels
    pub myplatform: Option<MyPlatformConfig>,
}
```

### 3. Register the Channel

Open `src/channels/mod.rs` and make the following changes:

1.  **Expose the module**:
    ```rust
    pub mod myplatform;
    pub use myplatform::MyPlatformChannel;
    ```

2.  **Update `doctor_channels`**:
    Add a health check entry for your channel.
    ```rust
    if let Some(ref config) = config.channels_config.myplatform {
        channels.push((
            "MyPlatform",
            Arc::new(MyPlatformChannel::new(/* pass config fields */)),
        ));
    }
    ```

3.  **Update `start_channels`**:
    Initialize and start your channel.
    ```rust
    if let Some(ref config) = config.channels_config.myplatform {
        channels.push(Arc::new(MyPlatformChannel::new(/* pass config fields */)));
    }
    ```

4.  **Update `handle_command` (List)**:
    Add your channel to the `List` command output.
    ```rust
    ("MyPlatform", config.channels_config.myplatform.is_some()),
    ```

## Testing

1.  **Unit Tests**: Add tests in your module file (`src/channels/myplatform.rs`) to verify logic (e.g., parsing, config handling).
2.  **Integration Test**:
    *   Add configuration to your local `config.toml` (or env vars).
    *   Run `cargo run channel doctor` to verify health check.
    *   Run `cargo run start` to test sending/receiving messages.

## Best Practices

*   **Error Handling**: Use `anyhow::Result` for fallible operations.
*   **Async**: Use `async_trait` and valid async runtimes (Tokio).
*   **Graceful Shutdown**: The `listen` loop should handle shutdown signals if possible, or simply return on error/channel close.
*   **Rate Limiting**: Respect platform rate limits to avoid bans.
