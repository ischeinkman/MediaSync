pub fn array_copy<Itm: Copy, Col: Default + AsRef<[Itm]> + AsMut<[Itm]>>(src: &[Itm]) -> Col {
    let mut retvl = Col::default();
    let retvl_ref = retvl.as_mut();
    let len = retvl_ref.len();
    retvl_ref.copy_from_slice(&src[0..len]);
    retvl
}
pub trait AbsSub {
    type Output;
    fn abs_sub(self, other: Self) -> Self::Output;
}

impl AbsSub for u64 {
    type Output = Self;
    fn abs_sub(self, other: Self) -> Self {
        if self > other {
            self - other
        } else {
            other - self
        }
    }
}
