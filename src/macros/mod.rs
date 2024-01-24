use serde::{Deserialize, Serialize};

pub mod route;

pub trait RequestMethod {
    type OkSend<'a>: Serialize;
    type OkRecv<'de>: Deserialize<'de>;
    type ErrSend<'a>: Serialize;
    type ErrRecv<'de>: Deserialize<'de>;
}

pub trait NotifyMethod {
    type ParamSend<'a>: Serialize;
    type ParamRecv<'de>: Deserialize<'de>;

    const METHOD_NAME: &'static str;
}

/// Tries to verify if we can generate following code
#[test]
#[ignore]
#[cfg(feature = "jsonrpc")]
fn xx() {
    #![allow(unused_parens)]

    #[derive(Serialize)]
    struct Foo<'a> {
        x: &'a str,
    }
    #[derive(Serialize, Deserialize)]
    struct A {
        i: i32,
        vv: f32,
        k: Vec<i32>,
    }

    impl<'de> serde::Deserialize<'de> for Foo<'de> {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            let (x) = <(&str) as serde::Deserialize>::deserialize(deserializer)?;

            Ok(Self { x })
        }
    }

    struct MyMethod;

    impl NotifyMethod for MyMethod {
        type ParamSend<'a> = Foo<'a>;
        type ParamRecv<'de> = Foo<'de>;

        const METHOD_NAME: &'static str = "my_method";
    }

    let payload = "{}";
    let _ = serde_json::from_str::<<MyMethod as NotifyMethod>::ParamRecv<'_>>(payload);
}

pub mod inbound;
