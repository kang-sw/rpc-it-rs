#![allow(unused)]

use std::borrow::{Borrow, Cow};

#[rpc_it_macros::service(rename_all = "camelCase")]
pub(crate) mod prods {
    fn foo_my_dlfofl();

    fn bocci_chan(kawaii: __<String, (i32, i32)>) -> i32;

    /// dasf
    /// sagfa gsda
    #[name = "dslaoi"]
    fn eoo(s: __<i32, usize>, g: f32);

    // fn bar(my_name: &str, is: MyArg<'_>);

    fn baz(john: String, doe: Cow<'_, [i32]>) -> i32;

    fn tew(te: String, go: Tew) -> i32;

    fn qux(las: i32, ggg: i32) -> Result<__<&'_ str, String>, i32>;
}

#[rpc_it_macros::service(rename_all = "kebab-case", no_recv)]
mod ga {
    fn foo_my_dlfofl();

    /// dasf
    /// sagfa gsda
    #[name = "dslaoi"]
    fn eoo(s: __<i32, usize>, g: f32);

    // fn bar(my_name: &str, is: MyArg<'_>);

    fn baz(john: String, doe: __<&[i32], Vec<i32>>) -> i32;

    fn qux(las: String, ggg: i32) -> Result<&'_ str, i32>;
}

use ga::*;

#[derive(serde::Serialize, serde::Deserialize)]
struct MyArg<'a> {
    #[serde(borrow)]
    key: Cow<'a, str>,
}

#[derive(Default, Clone, Copy, serde::Serialize, serde::Deserialize)]
struct Tew {
    value: [i32; 10],
}

#[derive(serde::Deserialize)]
struct Nt(i32);

fn gore<'a>(_: &impl Borrow<MyArg<'a>>, _: &impl Borrow<Tew>) {}

fn __compile_test() {
    #![allow(invalid_value)]

    let handle: rpc_it::RequestSender<()> = unsafe { std::mem::zeroed() };

    let b = &mut Default::default();
    handle.try_noti(b, eoo(&32, &4.1)).ok();

    let req: rpc_it::cached::Request<(), prods::tew::Fn> = unsafe { std::mem::zeroed() };
    let _ = req.args().go.value;
    req.try_response(b, Ok(&32)).ok();

    let req: rpc_it::cached::Request<(), prods::bocci_chan::Fn> = unsafe { std::mem::zeroed() };
    let _val = req.args().1;
    req.try_response(b, Ok(&32)).ok();
}
