#![cfg(feature = "proc-macro-rkyv")]

#[derive(rkyv::Archive)]
struct Foo<'a> {
    #[with(rkyv::with::AsOwned)]
    i: &'a str,
}

fn __compiled() {
    let buf = &b"fds"[..];
    let x = unsafe { rkyv::archived_root::<Foo>(buf) };
}
