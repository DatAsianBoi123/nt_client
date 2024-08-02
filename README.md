# nt_client

[![downloads](https://img.shields.io/crates/v/nt_client?style=for-the-badge)](https://crates.io/crates/nt_client)
[![crates.io](https://img.shields.io/crates/d/nt_client?style=for-the-badge)](https://crates.io/crates/nt_client)
[![docs](https://img.shields.io/badge/docs-nt__client-CE412B?style=for-the-badge)](https://docs.rs/nt_client/latest/nt_client)
[![license](https://img.shields.io/crates/l/nt_client?style=for-the-badge)](https://opensource.org/license/mit)

A blazingly fast [WPI NetworkTables 4.1](https://github.com/wpilibsuite/allwpilib/blob/main/ntcore/doc/networktables4.adoc) client, written in Rust.

This is meant to be used within coprocessors that can send and receive data from the robot.

**This is still a pre-1.0.0 release! Expect things to not work correctly and breaking changes until a full 1.0.0 release is available!**

## 1.0.0 Release Checklist
- [x] Connecting to server
- [x] Subscribing to a topic
- [x] Publishing to a topic
- [ ] 100% documentation coverage (12.41%)
- [ ] Proper logging (instead of println!)
- [ ] Examples
- [ ] Better error handling (less `.expect`)
- [ ] Reconnections
- [ ] Caching

## Installation
Add the following dependency to your `Cargo.toml`
```toml
nt_client = "0.1.0"
```
Or run the following command in your project root
```
cargo add nt_client
```
## Example
A basic subscriber that prints changes to `stdout`

```rust
use nt_client::{Client, NewClientOptions, NTAddr};

#[tokio::main]
async fn main() {
    let options = NewClientOptions { addr: NTAddr::Local, ..Default::default() };
    let client = Client::new(options);

    let thing_topic = client.topic("/thing");
    tokio::spawn(async move {
        let mut sub = thing_topic.subscribe::<String>(Default::default())
            .await
            .expect("websocket connection closed!");

        while let Ok(recv) = sub.recv().await {
            println!("topic updated: '{recv}'")
        }
    });

    client.connect().await.unwrap();
});
```

