// Recursive expansion of service macro
// =====================================

mod rpc {
    #![allow(unused_parens)]
    #![allow(unused)]
    #![allow(non_camel_case_types)]
    #![allow(non_snake_case)]
    #![allow(clippy::needless_lifetimes)]
    use super::*;
    use ___crate::macros as ___macros;
    use ___crate::serde;
    use ___macros::route as ___route;
    use rpc_it as ___crate;
    pub(super) enum Route<R: ___crate::Config> {
        ZeroParam(___crate::cached::Request<R, self::zero_param::Fn>),
        OneParamFlip(___crate::cached::Request<R, self::one_param_flip::Fn>),
        TwoParamAdd(___crate::cached::Request<R, self::two_param_add::Fn>),
        ThreeParamConcat(___crate::cached::Request<R, self::three_param_concat::Fn>),
        FourParamConcat(___crate::cached::Request<R, self::four_param_concat::Fn>),
        OneParamSplit(___crate::cached::Request<R, self::one_param_split::Fn>),
    }
    impl<R: ___crate::Config> Route<R> {
        pub(super) fn install<B, E>(
            ___router: &mut ___route::RouterBuilder<R, B>,
            ___handler: impl Fn(Self) + Send + Sync + 'static + Clone,
            mut ___on_route_error: impl FnMut(&str, &str, B::Error) -> E,
        ) where
            B: ___route::RouterFuncBuilder,
            E: Into<___route::RouteFailResponse>,
        {
            use ___route::RouteFailResponse as ___RF;
            {
                let ___handler = ___handler.clone();
                ___router.push_handler(move |___inbound| {
                    let ___ib = ___inbound.take().unwrap();
                    #[allow(unsafe_code)]
                    unsafe {
                        ___handler(Route::ZeroParam(
                            ___crate::cached::Request::__internal_create(___ib).map_err(
                                |(___ib, ___err)| {
                                    *___inbound = Some(___ib);
                                    ___err
                                },
                            )?,
                        ));
                    }
                    Ok(())
                });
                let ___routes = ["zero_param"];
                for ___route in ___routes {
                    let Err(___err) = ___router.try_add_route_to_last(___route) else {
                        continue;
                    };
                    match ___on_route_error("zero_param", ___route, ___err).into() {
                        ___RF::IgnoreAndContinue => {}

                        ___RF::Panic => {
                            panic!("Failed to add route '{}::{}'", "zero_param", ___route);
                        }
                        ___RF::Abort => return,
                    }
                }
            }
            {
                let ___handler = ___handler.clone();
                ___router.push_handler(move |___inbound| {
                    let ___ib = ___inbound.take().unwrap();
                    #[allow(unsafe_code)]
                    unsafe {
                        ___handler(Route::OneParamFlip(
                            ___crate::cached::Request::__internal_create(___ib).map_err(
                                |(___ib, ___err)| {
                                    *___inbound = Some(___ib);
                                    ___err
                                },
                            )?,
                        ));
                    }
                    Ok(())
                });
                let ___routes = ["One/Param/Flip"];
                for ___route in ___routes {
                    let Err(___err) = ___router.try_add_route_to_last(___route) else {
                        continue;
                    };
                    match ___on_route_error("one_param_flip", ___route, ___err).into() {
                        ___RF::IgnoreAndContinue => {}

                        ___RF::Panic => {
                            panic!("Failed to add route '{}::{}'", "one_param_flip", ___route);
                        }
                        ___RF::Abort => return,
                    }
                }
            }
            {
                let ___handler = ___handler.clone();
                ___router.push_handler(move |___inbound| {
                    let ___ib = ___inbound.take().unwrap();
                    #[allow(unsafe_code)]
                    unsafe {
                        ___handler(Route::TwoParamAdd(
                            ___crate::cached::Request::__internal_create(___ib).map_err(
                                |(___ib, ___err)| {
                                    *___inbound = Some(___ib);
                                    ___err
                                },
                            )?,
                        ));
                    }
                    Ok(())
                });
                let ___routes = ["T wo P aaa ram Ad"];
                for ___route in ___routes {
                    let Err(___err) = ___router.try_add_route_to_last(___route) else {
                        continue;
                    };
                    match ___on_route_error("two_param_add", ___route, ___err).into() {
                        ___RF::IgnoreAndContinue => {}

                        ___RF::Panic => {
                            panic!("Failed to add route '{}::{}'", "two_param_add", ___route);
                        }
                        ___RF::Abort => return,
                    }
                }
            }
            {
                let ___handler = ___handler.clone();
                ___router.push_handler(move |___inbound| {
                    let ___ib = ___inbound.take().unwrap();
                    #[allow(unsafe_code)]
                    unsafe {
                        ___handler(Route::ThreeParamConcat(
                            ___crate::cached::Request::__internal_create(___ib).map_err(
                                |(___ib, ___err)| {
                                    *___inbound = Some(___ib);
                                    ___err
                                },
                            )?,
                        ));
                    }
                    Ok(())
                });
                let ___routes = [
                    "three_param_concat",
                    "Param/Three",
                    "Param/Thre",
                    "Param/Thr",
                    "Param/Th",
                    "Param/T",
                ];
                for ___route in ___routes {
                    let Err(___err) = ___router.try_add_route_to_last(___route) else {
                        continue;
                    };
                    match ___on_route_error("three_param_concat", ___route, ___err).into() {
                        ___RF::IgnoreAndContinue => {}

                        ___RF::Panic => {
                            panic!(
                                "Failed to add route '{}::{}'",
                                "three_param_concat", ___route
                            );
                        }
                        ___RF::Abort => return,
                    }
                }
            }
            {
                let ___handler = ___handler.clone();
                ___router.push_handler(move |___inbound| {
                    let ___ib = ___inbound.take().unwrap();
                    #[allow(unsafe_code)]
                    unsafe {
                        ___handler(Route::FourParamConcat(
                            ___crate::cached::Request::__internal_create(___ib).map_err(
                                |(___ib, ___err)| {
                                    *___inbound = Some(___ib);
                                    ___err
                                },
                            )?,
                        ));
                    }
                    Ok(())
                });
                let ___routes = ["four_param_concat"];
                for ___route in ___routes {
                    let Err(___err) = ___router.try_add_route_to_last(___route) else {
                        continue;
                    };
                    match ___on_route_error("four_param_concat", ___route, ___err).into() {
                        ___RF::IgnoreAndContinue => {}

                        ___RF::Panic => {
                            panic!(
                                "Failed to add route '{}::{}'",
                                "four_param_concat", ___route
                            );
                        }
                        ___RF::Abort => return,
                    }
                }
            }
            {
                let ___handler = ___handler;
                ___router.push_handler(move |___inbound| {
                    let ___ib = ___inbound.take().unwrap();
                    #[allow(unsafe_code)]
                    unsafe {
                        ___handler(Route::OneParamSplit(
                            ___crate::cached::Request::__internal_create(___ib).map_err(
                                |(___ib, ___err)| {
                                    *___inbound = Some(___ib);
                                    ___err
                                },
                            )?,
                        ));
                    }
                    Ok(())
                });
                let ___routes = ["one_param_split"];
                for ___route in ___routes {
                    let Err(___err) = ___router.try_add_route_to_last(___route) else {
                        continue;
                    };
                    match ___on_route_error("one_param_split", ___route, ___err).into() {
                        ___RF::IgnoreAndContinue => {}

                        ___RF::Panic => {
                            panic!("Failed to add route '{}::{}'", "one_param_split", ___route);
                        }
                        ___RF::Abort => return,
                    }
                }
            }
        }
    }
    pub(super) enum DRoute<R: ___crate::Config> {
        ZeroParam(___crate::cached::Request<R, self::zero_param::Fn>),
        OneParamFlip(___crate::cached::Request<R, self::one_param_flip::Fn>),
        TwoParamAdd(___crate::cached::Request<R, self::two_param_add::Fn>),
        ThreeParamConcat(___crate::cached::Request<R, self::three_param_concat::Fn>),
        FourParamConcat(___crate::cached::Request<R, self::four_param_concat::Fn>),
        OneParamSplit(___crate::cached::Request<R, self::one_param_split::Fn>),
    }
    impl<R: ___crate::Config> DRoute<R> {
        pub(super) fn install<B, E>(
            ___router: &mut ___route::RouterBuilder<R, B>,
            ___handler: impl Fn(Self) + Send + Sync + 'static + Clone,
            mut ___on_route_error: impl FnMut(&str, &str, B::Error) -> E,
        ) where
            B: ___route::RouterFuncBuilder,
            E: Into<___route::RouteFailResponse>,
        {
            use ___route::RouteFailResponse as ___RF;
            {
                let ___handler = ___handler.clone();
                ___router.push_handler(move |___inbound| {
                    let ___ib = ___inbound.take().unwrap();
                    #[allow(unsafe_code)]
                    unsafe {
                        ___handler(DRoute::ZeroParam(
                            ___crate::cached::Request::__internal_create(___ib).map_err(
                                |(___ib, ___err)| {
                                    *___inbound = Some(___ib);
                                    ___err
                                },
                            )?,
                        ));
                    }
                    Ok(())
                });
                let ___routes = ["zero_param"];
                for ___route in ___routes {
                    let Err(___err) = ___router.try_add_route_to_last(___route) else {
                        continue;
                    };
                    match ___on_route_error("zero_param", ___route, ___err).into() {
                        ___RF::IgnoreAndContinue => {}

                        ___RF::Panic => {
                            panic!("Failed to add route '{}::{}'", "zero_param", ___route);
                        }
                        ___RF::Abort => return,
                    }
                }
            }
            {
                let ___handler = ___handler.clone();
                ___router.push_handler(move |___inbound| {
                    let ___ib = ___inbound.take().unwrap();
                    #[allow(unsafe_code)]
                    unsafe {
                        ___handler(DRoute::OneParamFlip(
                            ___crate::cached::Request::__internal_create(___ib).map_err(
                                |(___ib, ___err)| {
                                    *___inbound = Some(___ib);
                                    ___err
                                },
                            )?,
                        ));
                    }
                    Ok(())
                });
                let ___routes = ["One/Param/Flip"];
                for ___route in ___routes {
                    let Err(___err) = ___router.try_add_route_to_last(___route) else {
                        continue;
                    };
                    match ___on_route_error("one_param_flip", ___route, ___err).into() {
                        ___RF::IgnoreAndContinue => {}

                        ___RF::Panic => {
                            panic!("Failed to add route '{}::{}'", "one_param_flip", ___route);
                        }
                        ___RF::Abort => return,
                    }
                }
            }
            {
                let ___handler = ___handler.clone();
                ___router.push_handler(move |___inbound| {
                    let ___ib = ___inbound.take().unwrap();
                    #[allow(unsafe_code)]
                    unsafe {
                        ___handler(DRoute::TwoParamAdd(
                            ___crate::cached::Request::__internal_create(___ib).map_err(
                                |(___ib, ___err)| {
                                    *___inbound = Some(___ib);
                                    ___err
                                },
                            )?,
                        ));
                    }
                    Ok(())
                });
                let ___routes = ["T wo P aaa ram Ad"];
                for ___route in ___routes {
                    let Err(___err) = ___router.try_add_route_to_last(___route) else {
                        continue;
                    };
                    match ___on_route_error("two_param_add", ___route, ___err).into() {
                        ___RF::IgnoreAndContinue => {}

                        ___RF::Panic => {
                            panic!("Failed to add route '{}::{}'", "two_param_add", ___route);
                        }
                        ___RF::Abort => return,
                    }
                }
            }
            {
                let ___handler = ___handler.clone();
                ___router.push_handler(move |___inbound| {
                    let ___ib = ___inbound.take().unwrap();
                    #[allow(unsafe_code)]
                    unsafe {
                        ___handler(DRoute::ThreeParamConcat(
                            ___crate::cached::Request::__internal_create(___ib).map_err(
                                |(___ib, ___err)| {
                                    *___inbound = Some(___ib);
                                    ___err
                                },
                            )?,
                        ));
                    }
                    Ok(())
                });
                let ___routes = [
                    "three_param_concat",
                    "Param/Three",
                    "Param/Thre",
                    "Param/Thr",
                    "Param/Th",
                    "Param/T",
                ];
                for ___route in ___routes {
                    let Err(___err) = ___router.try_add_route_to_last(___route) else {
                        continue;
                    };
                    match ___on_route_error("three_param_concat", ___route, ___err).into() {
                        ___RF::IgnoreAndContinue => {}

                        ___RF::Panic => {
                            panic!(
                                "Failed to add route '{}::{}'",
                                "three_param_concat", ___route
                            );
                        }
                        ___RF::Abort => return,
                    }
                }
            }
            {
                let ___handler = ___handler.clone();
                ___router.push_handler(move |___inbound| {
                    let ___ib = ___inbound.take().unwrap();
                    #[allow(unsafe_code)]
                    unsafe {
                        ___handler(DRoute::FourParamConcat(
                            ___crate::cached::Request::__internal_create(___ib).map_err(
                                |(___ib, ___err)| {
                                    *___inbound = Some(___ib);
                                    ___err
                                },
                            )?,
                        ));
                    }
                    Ok(())
                });
                let ___routes = ["four_param_concat"];
                for ___route in ___routes {
                    let Err(___err) = ___router.try_add_route_to_last(___route) else {
                        continue;
                    };
                    match ___on_route_error("four_param_concat", ___route, ___err).into() {
                        ___RF::IgnoreAndContinue => {}

                        ___RF::Panic => {
                            panic!(
                                "Failed to add route '{}::{}'",
                                "four_param_concat", ___route
                            );
                        }
                        ___RF::Abort => return,
                    }
                }
            }
            {
                let ___handler = ___handler;
                ___router.push_handler(move |___inbound| {
                    let ___ib = ___inbound.take().unwrap();
                    #[allow(unsafe_code)]
                    unsafe {
                        ___handler(DRoute::OneParamSplit(
                            ___crate::cached::Request::__internal_create(___ib).map_err(
                                |(___ib, ___err)| {
                                    *___inbound = Some(___ib);
                                    ___err
                                },
                            )?,
                        ));
                    }
                    Ok(())
                });
                let ___routes = ["one_param_split"];
                for ___route in ___routes {
                    let Err(___err) = ___router.try_add_route_to_last(___route) else {
                        continue;
                    };
                    match ___on_route_error("one_param_split", ___route, ___err).into() {
                        ___RF::IgnoreAndContinue => {}

                        ___RF::Panic => {
                            panic!("Failed to add route '{}::{}'", "one_param_split", ___route);
                        }
                        ___RF::Abort => return,
                    }
                }
            }
        }
        fn ___inner_match_index(___route: &str) -> Option<usize> {
            Some(match ___route {
                "zero_param" => 0usize,
                "One/Param/Flip" => 1usize,
                "T wo P aaa ram Ad" => 2usize,
                "three_param_concat" | "Param/Three" | "Param/Thre" | "Param/Thr" | "Param/Th"
                | "Param/T" => 3usize,
                "four_param_concat" => 4usize,
                "one_param_split" => 5usize,
                _ => return None,
            })
        }
        fn matches(___route: &str) -> bool {
            Self::___inner_match_index(___route).is_some()
        }
        fn route(
            ___ib: ___crate::Inbound<R>,
        ) -> Result<Self, (___crate::Inbound<R>, ___route::ExecError)> {
            let Some(___index) = Self::___inner_match_index(___ib.method()) else {
                return Err((___ib, ___route::ExecError::RouteFailed));
            };
            #[allow(unsafe_code)]
            unsafe {
                let ___item = match ___index {
          0usize => Self::ZeroParam(___crate::cached::Request:: <R,self::zero_param::Fn> ::__internal_create(___ib).map_err(|(___ib,___err)|{
            (___ib,___err.into())
          })?),
          1usize => Self::OneParamFlip(___crate::cached::Request:: <R,self::one_param_flip::Fn> ::__internal_create(___ib).map_err(|(___ib,___err)|{
            (___ib,___err.into())
          })?),
          2usize => Self::TwoParamAdd(___crate::cached::Request:: <R,self::two_param_add::Fn> ::__internal_create(___ib).map_err(|(___ib,___err)|{
            (___ib,___err.into())
          })?),
          3usize => Self::ThreeParamConcat(___crate::cached::Request:: <R,self::three_param_concat::Fn> ::__internal_create(___ib).map_err(|(___ib,___err)|{
            (___ib,___err.into())
          })?),
          4usize => Self::FourParamConcat(___crate::cached::Request:: <R,self::four_param_concat::Fn> ::__internal_create(___ib).map_err(|(___ib,___err)|{
            (___ib,___err.into())
          })?),
          5usize => Self::OneParamSplit(___crate::cached::Request:: <R,self::one_param_split::Fn> ::__internal_create(___ib).map_err(|(___ib,___err)|{
            (___ib,___err.into())
          })?),
          _ => unreachable!(),
        
          };
                Ok(___item)
            }
        }
    }
    pub mod zero_param {
        use super::*;
        pub struct Fn;

        impl ___macros::NotifyMethod for Fn {
            type ParamSend<'___ser> = ();
            type ParamRecv<'___de> = ();
            const METHOD_NAME: &'static str = "zero_param";
        }
        impl ___macros::RequestMethod for Fn {
            type OkSend<'___ser> = &'___ser i32;
            type OkRecv<'___de> = i32;
            type ErrSend<'___ser> = ();
            type ErrRecv<'___de> = ();
        }
    }
    pub fn zero_param<'___ser>() -> (
        zero_param::Fn,
        <zero_param::Fn as rpc_it::macros::NotifyMethod>::ParamSend<'___ser>,
    ) {
        (zero_param::Fn, ())
    }
    pub mod one_param_flip {
        use super::*;
        pub struct Fn;

        impl ___macros::NotifyMethod for Fn {
            type ParamSend<'___ser> = (&'___ser i32);
            type ParamRecv<'___de> = ParamRecv;
            const METHOD_NAME: &'static str = "One/Param/Flip";
        }
        #[derive(serde::Deserialize)]
        pub struct ParamRecv(i32);

        impl ::std::ops::Deref for ParamRecv {
            type Target = i32;
            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }
        impl ___macros::RequestMethod for Fn {
            type OkSend<'___ser> = &'___ser i32;
            type OkRecv<'___de> = i32;
            type ErrSend<'___ser> = ();
            type ErrRecv<'___de> = ();
        }
    }
    pub fn one_param_flip<'___ser>(
        a: &'___ser i32,
    ) -> (
        one_param_flip::Fn,
        <one_param_flip::Fn as rpc_it::macros::NotifyMethod>::ParamSend<'___ser>,
    ) {
        (one_param_flip::Fn, (a))
    }
    pub mod two_param_add {
        use super::*;
        pub struct Fn;

        impl ___macros::NotifyMethod for Fn {
            type ParamSend<'___ser> = (&'___ser i32, &'___ser i32);
            type ParamRecv<'___de> = self::ParamRecv;
            const METHOD_NAME: &'static str = "T wo P aaa ram Ad";
        }
        #[derive(serde::Deserialize)]
        pub struct ParamRecv {
            pub a: i32,
            pub b: i32,
        }
        impl ___macros::RequestMethod for Fn {
            type OkSend<'___ser> = &'___ser i32;
            type OkRecv<'___de> = i32;
            type ErrSend<'___ser> = ();
            type ErrRecv<'___de> = ();
        }
    }
    pub fn two_param_add<'___ser>(
        a: &'___ser i32,
        b: &'___ser i32,
    ) -> (
        two_param_add::Fn,
        <two_param_add::Fn as rpc_it::macros::NotifyMethod>::ParamSend<'___ser>,
    ) {
        (two_param_add::Fn, (a, b))
    }
    pub mod three_param_concat {
        use super::*;
        pub struct Fn;

        impl ___macros::NotifyMethod for Fn {
            type ParamSend<'___ser> = (&'___ser String, &'___ser String, &'___ser String);
            type ParamRecv<'___de> = self::ParamRecv;
            const METHOD_NAME: &'static str = "three_param_concat";
        }
        #[derive(serde::Deserialize)]
        pub struct ParamRecv {
            pub a: String,
            pub b: String,
            pub c: String,
        }
        impl ___macros::RequestMethod for Fn {
            type OkSend<'___ser> = &'___ser String;
            type OkRecv<'___de> = String;
            type ErrSend<'___ser> = ();
            type ErrRecv<'___de> = ();
        }
    }
    pub fn three_param_concat<'___ser>(
        a: &'___ser String,
        b: &'___ser String,
        c: &'___ser String,
    ) -> (
        three_param_concat::Fn,
        <three_param_concat::Fn as rpc_it::macros::NotifyMethod>::ParamSend<'___ser>,
    ) {
        (three_param_concat::Fn, (a, b, c))
    }
    pub mod four_param_concat {
        use super::*;
        pub struct Fn;

        impl ___macros::NotifyMethod for Fn {
            type ParamSend<'___ser> =
                (&'___ser String, &'___ser i32, &'___ser u64, &'___ser String);
            type ParamRecv<'___de> = self::ParamRecv;
            const METHOD_NAME: &'static str = "four_param_concat";
        }
        #[derive(serde::Deserialize)]
        pub struct ParamRecv {
            pub a: String,
            pub b: i32,
            pub c: u64,
            pub d: String,
        }
        impl ___macros::RequestMethod for Fn {
            type OkSend<'___ser> = &'___ser String;
            type OkRecv<'___de> = String;
            type ErrSend<'___ser> = ();
            type ErrRecv<'___de> = ();
        }
    }
    pub fn four_param_concat<'___ser>(
        a: &'___ser String,
        b: &'___ser i32,
        c: &'___ser u64,
        d: &'___ser String,
    ) -> (
        four_param_concat::Fn,
        <four_param_concat::Fn as rpc_it::macros::NotifyMethod>::ParamSend<'___ser>,
    ) {
        (four_param_concat::Fn, (a, b, c, d))
    }
    pub mod one_param_split {
        use super::*;
        pub struct Fn;

        impl ___macros::NotifyMethod for Fn {
            type ParamSend<'___ser> = (&'___ser i32);
            type ParamRecv<'___de> = ParamRecv;
            const METHOD_NAME: &'static str = "one_param_split";
        }
        #[derive(serde::Deserialize)]
        pub struct ParamRecv(f32);

        impl ::std::ops::Deref for ParamRecv {
            type Target = f32;
            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }
        impl ___macros::RequestMethod for Fn {
            type OkSend<'___ser> = &'___ser i32;
            type OkRecv<'___de> = i32;
            type ErrSend<'___ser> = ();
            type ErrRecv<'___de> = ();
        }
    }
    pub fn one_param_split<'___ser>(
        x: &'___ser i32,
    ) -> (
        one_param_split::Fn,
        <one_param_split::Fn as rpc_it::macros::NotifyMethod>::ParamSend<'___ser>,
    ) {
        (one_param_split::Fn, (x))
    }
}
