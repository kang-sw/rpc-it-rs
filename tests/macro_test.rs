use std::borrow::{Borrow, Cow};

use hashbrown::HashMap;

#[rpc_it_macros::service(rename_all = "camelCase")]
extern "prods" {
    fn foo_my_dlfofl();

    fn bocci_chan(kawaii: __<String, (i32, i32)>);

    /// dasf
    /// sagfa gsda
    #[name = "dslaoi"]
    fn eoo(s: __<i32, usize>, g: f32);

    // fn bar(my_name: &str, is: MyArg<'_>);

    fn baz(john: String, doe: Cow<'_, [i32]>) -> i32;

    fn qux(las: i32, ggg: i32) -> Result<__<&'_ str, String>, i32>;
}

#[rpc_it_macros::service(flatten, rename_all = "kebab-case")]
extern "" {
    fn foo_my_dlfofl();

    /// dasf
    /// sagfa gsda
    #[name = "dslaoi"]
    fn eoo(s: __<i32, usize>, g: f32);

    // fn bar(my_name: &str, is: MyArg<'_>);

    fn baz(john: String, doe: __<&[i32], Vec<i32>>) -> i32;

    fn qux(las: String, ggg: i32) -> Result<&'_ str, i32>;
}

#[derive(serde::Serialize, serde::Deserialize)]
struct MyArg<'a> {
    #[serde(borrow)]
    key: Cow<'a, str>,
}

#[derive(Default, Clone, Copy)]
struct Tew {
    value: [i32; 10],
}

fn gore<'a>(_: &impl Borrow<MyArg<'a>>, _: &impl Borrow<Tew>) {}

fn __compile_test() {
    #![allow(invalid_value)]

    let handle: rpc_it::RequestSender<()> = unsafe { std::mem::zeroed() };

    let b = &mut Default::default();
    handle.try_noti(b, eoo(&32, &4.1)).ok();

    let req: rpc_it::cached::Request<(), prods::qux::Fn> = unsafe { std::mem::zeroed() };
    req.args();
    req.try_response(b, Ok("hello!")).ok();

    let service = rpc_it::RouterBuilder::<(), HashMap<String, usize>>::default();
}
