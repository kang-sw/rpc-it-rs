#![allow(unused)]

use std::borrow::{Borrow, Cow};

#[rpc_it_macros::service(rename_all = "camelCase", no_param_recv_newtype)]
pub(crate) mod prods {

    const MyRoute: Route = ALL_PASCAL_CASE;

    const OtherRoute: Route = [
        // bocci_chan = AliasName {
        //     routes: ["a", "b", "c"],
        // },
        // eoo {
        //     routes: ["a", "b", "c", "D"],
        // },
        // bax = OtherAlias,
        qux,
    ];

    fn foo_my_dlfofl();

    fn godsa(_: i32);

    fn mono_ser_de_param(pff: __<String, (i32, i32)>) -> i32;

    /// dasf
    /// sagfa gsda
    #[name = "dslaoi"]
    #[route = "hey dude"]
    fn eoo(s: __<i32, usize>, g: f32);

    // fn bar(my_name: &str, is: MyArg<'_>);

    fn baz(john: String, doe: Cow<'_, [i32]>) -> i32;

    fn qux(las: i32, ggg: i32) -> Result<__<&'_ str, String>, i32>;
}
