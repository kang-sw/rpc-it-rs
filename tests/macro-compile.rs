#[rpc_it_macros::service]
pub trait TestService {
    fn add(&self, a: u32, b: u32) -> u32 {
        a + b
    }

    fn try_add(a: u32, b: u32) -> Result<u32, ()> {
        Ok(a + b)
    }

    #[sync]
    fn add_sync(&self, a: u32, b: u32) -> u32 {
        a + b
    }
}

#[derive(Clone)]
struct ImplSvc {}

impl test_service::Service for ImplSvc {
    fn add(&self, _: &dyn rpc_it::UserData, a: u32, b: u32, ___rq: rpc_it::TypedRequest<u32, ()>) {
        let ___returned_value = (move || a + b)();
        ___rq.ok(&___returned_value).ok();
    }

    fn try_add(_: &dyn rpc_it::UserData, a: u32, b: u32, ___rq: rpc_it::TypedRequest<u32, ()>) {
        let ___returned_value = (move || Ok(a + b))();
        match ___returned_value {
            Ok(x) => ___rq.ok(&x).ok(),
            Err(e) => ___rq.err(&e).ok(),
        };
    }

    fn add_sync(&self, _: &dyn rpc_it::UserData, a: u32, b: u32) -> u32 {
        a + b
    }
}
