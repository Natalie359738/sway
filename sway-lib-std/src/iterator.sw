library iterator;

pub trait Iterator {
    type Item;

    fn next(self) -> Option<Self::Item>;
}