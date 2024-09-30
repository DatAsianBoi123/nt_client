//! An example of changing topic properties.

use std::{collections::HashMap, time::Duration};

use nt_client::{data::Properties, publish::SetPropsBuilder, Client};

#[tokio::main]
async fn main() {
    let client = Client::new(Default::default());

    let topic = client.topic("/mytopic");
    tokio::spawn(async move {
        let mut extra_props = HashMap::new();
        extra_props.insert("custom".to_string(), "something".to_string());

        // initial properties are:
        // - cached: true
        // - custom: `something`
        // everything else is unset
        let mut sub = topic.publish::<String>(Properties { cached: Some(true), extra: extra_props, ..Default::default() }).await.unwrap();

        // after 1 second...
        tokio::time::sleep(Duration::from_secs(1)).await;

        let updated = SetPropsBuilder::new()
            .replace_persistent(true)
            .delete("custom".to_string())
            .build();

        // update properties
        // updated properties are:
        // - cached: true
        // - persistent: true
        // everything else is unset (including the `custom` property)
        sub.update_props(updated).await.unwrap();
    });

    client.connect().await.unwrap()
}

