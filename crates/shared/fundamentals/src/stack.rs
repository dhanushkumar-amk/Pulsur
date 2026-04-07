// Program 2: Stack implementation with push, pop, peek using generics.
// Demonstrates how to write generic code that works for any type T.

#[derive(Debug)]
pub struct Stack<T> {
    items: Vec<T>,
}

impl<T> Stack<T> {
    pub fn new() -> Self {
        Self { items: Vec::new() }
    }

    pub fn push(&mut self, item: T) {
        self.items.push(item);
    }

    pub fn pop(&mut self) -> Option<T> {
        self.items.pop()
    }

    pub fn peek(&self) -> Option<&T> {
        self.items.last()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }
}

// Default implementation for Stack
impl<T> Default for Stack<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stack_new() {
        let stack: Stack<i32> = Stack::new();
        assert!(stack.is_empty());
    }

    #[test]
    fn test_stack_push_pop() {
        let mut stack = Stack::new();
        stack.push("hello");
        stack.push("world");

        assert_eq!(stack.len(), 2);
        assert_eq!(stack.pop(), Some("world"));
        assert_eq!(stack.pop(), Some("hello"));
        assert_eq!(stack.pop(), None);
    }

    #[test]
    fn test_stack_peek() {
        let mut stack = Stack::new();
        stack.push(100);
        assert_eq!(stack.peek(), Some(&100));
        assert_eq!(stack.len(), 1);
    }

    #[test]
    fn test_stack_generic_complex() {
        let mut stack = Stack::new();
        stack.push(vec![1, 2, 3]);
        assert_eq!(stack.pop().unwrap().len(), 3);
    }

    #[test]
    fn test_stack_default() {
        let stack: Stack<u32> = Stack::default();
        assert!(stack.is_empty());
    }
}
