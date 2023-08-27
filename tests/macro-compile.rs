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

    #[sync]
    fn add_sync_2(&self, a: u32, b: u32) -> Result<u32, ()> {
        Ok(a + b)
    }

    fn noti(&self, _a: u32, _b: u32) {}

    fn noti_2(_a: u32, _b: u32) {}
}

#[derive(Clone)]
struct ImplSvc {}

impl test_service::Service for ImplSvc {
    fn add(&self, ___rq: rpc_it::TypedRequest<u32, ()>, a: u32, b: u32) {
        let ___returned_value = (move || a + b)();
        ___rq.ok(&___returned_value).ok();
    }

    fn try_add(___rq: rpc_it::TypedRequest<u32, ()>, a: u32, b: u32) {
        let ___returned_value = (move || Ok(a + b))();
        match ___returned_value {
            Ok(x) => ___rq.ok(&x).ok(),
            Err(e) => ___rq.err(&e).ok(),
        };
    }

    fn add_sync(&self, _: rpc_it::OwnedUserData, a: u32, b: u32) -> u32 {
        a + b
    }

    fn add_sync_2(&self, _: rpc_it::OwnedUserData, a: u32, b: u32) -> Result<u32, ()> {
        Ok(a + b)
    }

    fn noti(&self, _: rpc_it::Notify, _a: u32, _b: u32) {}

    fn noti_2(_: rpc_it::Notify, _a: u32, _b: u32) {}
}
