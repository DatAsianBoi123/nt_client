//! An example of using TopicPaths

use std::time::Duration;

use nt_client::{data::SubscriptionOptions, path, subscribe::ReceivedMessage, topic::TopicPath, Client};

#[tokio::main]
async fn main() {
    let client = Client::new(Default::default());

    // using the path! macro, we can create slash (/) delimited path names
    // path! macro evaluates to a TopicPath, and stringified it is `/my/long/path`
    let mut path = path!["my", "long", "path"];
    let sub_topic = client.topic(path.clone());
    // task that subscribes to that topic
    tokio::spawn(async move {
        // subscribes to that topic and only record topic announcements that have that prefix
        let mut subscriber = sub_topic.subscribe(SubscriptionOptions { topics_only: Some(true), prefix: Some(true), ..Default::default() }).await;

        while let Ok(ReceivedMessage::Announced(topic)) = subscriber.recv().await {
            // turn the topic name into a TopicPath
            let path: TopicPath = topic.name().into();
            // print the last segment in the path
            println!("topic announced, last segment is: {:?}", path.segments.back());
        }
    });

    // update the path and append `extra` to it
    // stringified, it is now `/my/long/path/extra`
    path.segments.push_back("extra".to_owned());
    let pub_topic = client.topic(path);
    // task that updates a counter to that topic every 1s
    tokio::spawn(async move {
        let publisher = pub_topic.publish::<u64>(Default::default()).await.unwrap();
        let mut interval = tokio::time::interval(Duration::from_secs(1));

        let mut counter = 0;
        loop {
            interval.tick().await;

            publisher.set(counter).await.unwrap();
            counter += 1;
        }
    });

    client.connect().await.unwrap()
}

