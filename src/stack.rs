use std::fmt::Debug;
use tracing_core::span::Id;

#[derive(Debug)]
struct IdValue<T> {
    id: Id,
    value: T,
}

#[derive(Debug)]
pub(crate) struct IdValueStack<T: Debug> {
    stack: Vec<IdValue<T>>,
}

impl<T: Debug> IdValueStack<T> {
    pub(crate) fn new() -> Self {
        IdValueStack { stack: Vec::new() }
    }

    #[inline]
    pub(crate) fn push(&mut self, id: Id, value: T) {
        self.stack.push(IdValue { id, value });
    }

    #[inline]
    pub(crate) fn pop(&mut self, id: &Id) -> Option<T> {
        if let Some((idx, _)) = self
            .stack
            .iter()
            .enumerate()
            .rev()
            .find(|(_, ctx_id)| ctx_id.id == *id)
        {
            let IdValue { id: _, value } = self.stack.remove(idx);
            return Some(value);
        }
        None
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.stack.len()
    }
}

#[cfg(test)]
mod tests {
    use super::{Id, IdValueStack};

    type IdStringStack = IdValueStack<String>;

    #[test]
    fn pop_last_value() {
        let mut stack = IdStringStack::new();
        let id1 = Id::from_u64(4711);
        stack.push(id1.clone(), String::from("one"));
        let id2 = Id::from_u64(1729);
        stack.push(id2.clone(), String::from("two"));
        assert_eq!(2, stack.len());

        assert_eq!(Some(String::from("two")), stack.pop(&id2));
        assert_eq!(1, stack.len());
        assert_eq!(Some(String::from("one")), stack.pop(&id1));
        assert_eq!(0, stack.len());
    }

    #[test]
    fn pop_first_value() {
        let mut stack = IdStringStack::new();
        let id1 = Id::from_u64(4711);
        stack.push(id1.clone(), String::from("one"));
        let id2 = Id::from_u64(1729);
        stack.push(id2.clone(), String::from("two"));

        assert_eq!(Some(String::from("one")), stack.pop(&id1));
        assert_eq!(1, stack.len());
        assert_eq!(Some(String::from("two")), stack.pop(&id2));
        assert_eq!(0, stack.len());
    }

    #[test]
    fn pop_middle_value() {
        let mut stack = IdStringStack::new();
        let id1 = Id::from_u64(4711);
        stack.push(id1.clone(), String::from("one"));
        let id2 = Id::from_u64(1729);
        stack.push(id2.clone(), String::from("two"));
        let id3 = Id::from_u64(1001);
        stack.push(id3.clone(), String::from("three"));

        assert_eq!(Some(String::from("three")), stack.pop(&id3));
        assert_eq!(2, stack.len());
        assert_eq!(Some(String::from("two")), stack.pop(&id2));
        assert_eq!(1, stack.len());
        assert_eq!(Some(String::from("one")), stack.pop(&id1));
        assert_eq!(0, stack.len());
    }
}
