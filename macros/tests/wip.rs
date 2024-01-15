use std::borrow::Cow;

#[rpc_it_macros::service]
extern "prods" {
    fn foo();

    fn eoo(s: __<i32, usize>, g: f32);

    // fn bar(my_name: &str, is: MyArg<'_>);

    // fn baz(john: String, doe: Cow<'_, [i32]>) -> i32;
}

#[derive(serde::Serialize, serde::Deserialize)]
struct MyArg<'a> {
    #[serde(borrow)]
    key: Cow<'a, str>,
}

fn __compile_test() {
    #[allow(invalid_value)]
    let handle: rpc_it::RequestSender<()> = unsafe { std::mem::zeroed() };

    let b = &mut Default::default();
    handle.try_noti(b, prods::eoo(&32, &4.1)).ok();
}
