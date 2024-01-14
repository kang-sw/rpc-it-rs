use std::borrow::Cow;

#[test]
#[ignore]
fn poc_serde_de() {
    let va = r#" "hello, world!" "#;
    let de = serde_json::from_str::<Cow<str>>(va).unwrap();

    assert!(matches!(de, Cow::Owned(_)))
}
