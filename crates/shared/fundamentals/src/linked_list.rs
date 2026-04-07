// Program 1: Linked list using Box<T> -- no Vec, raw recursive struct.
// Demonstrates memory management, ownership, and recursion depth control.

#[derive(Debug)]
pub struct LinkedList<T> {
    head: Option<Box<Node<T>>>,
}

#[derive(Debug)]
struct Node<T> {
    data: T,
    next: Option<Box<Node<T>>>,
}

impl<T> LinkedList<T> {
    pub fn new() -> Self {
        Self { head: None }
    }

    pub fn push_front(&mut self, data: T) {
        let new_node = Box::new(Node {
            data,
            next: self.head.take(),
        });
        self.head = Some(new_node);
    }

    pub fn pop_front(&mut self) -> Option<T> {
        self.head.take().map(|node| {
            self.head = node.next;
            node.data
        })
    }

    pub fn peek_front(&self) -> Option<&T> {
        self.head.as_ref().map(|node| &node.data)
    }

    pub fn is_empty(&self) -> bool {
        self.head.is_none()
    }

    pub fn len(&self) -> usize {
        let mut count = 0;
        let mut current = &self.head;
        while let Some(node) = current {
            count += 1;
            current = &node.next;
        }
        count
    }
}

impl<T> Default for LinkedList<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new() {
        let list: LinkedList<i32> = LinkedList::new();
        assert!(list.is_empty());
        assert_eq!(list.len(), 0);
    }

    #[test]
    fn test_push_pop() {
        let mut list = LinkedList::new();
        list.push_front(1);
        list.push_front(2);
        list.push_front(3);

        assert_eq!(list.len(), 3);
        assert_eq!(list.pop_front(), Some(3));
        assert_eq!(list.pop_front(), Some(2));
        assert_eq!(list.pop_front(), Some(1));
        assert_eq!(list.pop_front(), None);
    }

    #[test]
    fn test_peek() {
        let mut list = LinkedList::new();
        list.push_front(42);
        assert_eq!(list.peek_front(), Some(&42));
        list.pop_front();
        assert_eq!(list.peek_front(), None);
    }

    #[test]
    fn test_is_empty() {
        let mut list = LinkedList::new();
        assert!(list.is_empty());
        list.push_front(10);
        assert!(!list.is_empty());
        list.pop_front();
        assert!(list.is_empty());
    }

    #[test]
    fn test_len_recursive() {
        let mut list = LinkedList::new();
        for i in 0..100 {
            list.push_front(i);
        }
        assert_eq!(list.len(), 100);
    }
}
