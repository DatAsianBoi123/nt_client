use nt_client::{path, topic::TopicPath};

#[test]
fn test_single_item() {
    assert_eq!(into_path("Topic"), path!["Topic"]);
    assert_eq!(into_path("123thing"), path!["123thing"]);

    assert_eq!(into_path("/mydata"), path!["mydata"]);
    assert_eq!(into_path("value/"), path!["value"]);

    assert_eq!(into_path("//thing"), path!["/thing"]);
    assert_eq!(into_path("cooldata//"), path!["cooldata"]);
}

#[test]
fn test_multi_item() {
    assert_eq!(into_path("some/thing"), path!["some", "thing"]);
    assert_eq!(into_path("Topic/thing/value"), path!["Topic", "thing", "value"]);

    assert_eq!(into_path("/hello/there"), path!["hello", "there"]);
    assert_eq!(into_path("my/long/path/"), path!["my", "long", "path"]);

    assert_eq!(into_path("//weird///path/and/slash//"), path!["/weird", "/", "path", "and", "slash"]);
    assert_eq!(into_path("//////"), path!["/", "/"]);
}

#[test]
fn test_parse_to_string() {
    let path = path!["simple"];
    assert_eq!(into_path(&path.to_string()), path);

    let path = path!["my", "data"];
    assert_eq!(into_path(&path.to_string()), path);

    let path = path!["/something", "really", "/weird"];
    assert_eq!(into_path(&path.to_string()), path);
}

fn into_path(s: &str) -> TopicPath {
    s.into()
}

