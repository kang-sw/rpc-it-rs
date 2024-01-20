pub enum SerializeRkyv<'a, T: 'a + rkyv::Archive> {
    Original(&'a T),
}
