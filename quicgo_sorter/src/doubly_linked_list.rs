#![allow(dead_code)]

use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::ptr::NonNull;

struct Node<T> {
    prev: NonNull<Node<T>>,
    next: NonNull<Node<T>>,
    // Root sentinel of the list this node currently belongs to.
    root: Option<NonNull<Node<T>>>,
    value: Option<T>,
}

impl<T> Node<T> {
    fn new_detached(value: T) -> Self {
        Self {
            prev: NonNull::dangling(),
            next: NonNull::dangling(),
            root: None,
            value: Some(value),
        }
    }
}

pub(crate) struct Element<T> {
    ptr: NonNull<Node<T>>,
    marker: PhantomData<*const T>,
}

impl<T> Element<T> {
    #[inline]
    fn new(ptr: NonNull<Node<T>>) -> Self {
        Self {
            ptr,
            marker: PhantomData,
        }
    }
}

impl<T> Copy for Element<T> {}

impl<T> Clone for Element<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> PartialEq for Element<T> {
    fn eq(&self, other: &Self) -> bool {
        self.ptr == other.ptr
    }
}

impl<T> Eq for Element<T> {}

impl<T> Hash for Element<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.ptr.hash(state);
    }
}

impl<T> fmt::Debug for Element<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Element").field(&self.ptr).finish()
    }
}

/// Pointer-stable doubly linked list modeled after Go's list implementation.
///
/// The list owns a heap-allocated sentinel root and heap-allocated elements.
/// Element handles are raw-pointer wrappers and become invalid after removal.
/// Because Rust cannot track that lifetime automatically, methods that accept
/// an existing `Element<T>` handle are marked `unsafe`.
pub(crate) struct DoublyLinkedList<T> {
    root: NonNull<Node<T>>,
    len: usize,
    marker: PhantomData<Box<Node<T>>>,
}

impl<T> DoublyLinkedList<T> {
    pub(crate) fn new() -> Self {
        let root = Box::new(Node {
            prev: NonNull::dangling(),
            next: NonNull::dangling(),
            root: None,
            value: None,
        });

        let root = NonNull::new(Box::into_raw(root)).expect("Box::into_raw cannot return null");

        unsafe {
            (*root.as_ptr()).prev = root;
            (*root.as_ptr()).next = root;
            (*root.as_ptr()).root = Some(root);
        }

        Self {
            root,
            len: 0,
            marker: PhantomData,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub(crate) fn clear(&mut self) {
        while let Some(front) = self.front() {
            unsafe {
                let _ = self.remove(front);
            }
        }
    }

    pub(crate) fn init(&mut self) -> &mut Self {
        self.clear();
        self
    }

    pub(crate) fn front(&self) -> Option<Element<T>> {
        let next = unsafe { self.root.as_ref().next };
        if next == self.root {
            None
        } else {
            Some(Element::new(next))
        }
    }

    pub(crate) fn back(&self) -> Option<Element<T>> {
        let prev = unsafe { self.root.as_ref().prev };
        if prev == self.root {
            None
        } else {
            Some(Element::new(prev))
        }
    }

    pub(crate) fn push_front(&mut self, value: T) -> Element<T> {
        let node = self.alloc(value);

        unsafe {
            self.link_after(node, self.root);
        }

        self.len += 1;
        Element::new(node)
    }

    pub(crate) fn push_back(&mut self, value: T) -> Element<T> {
        let node = self.alloc(value);
        let at = unsafe { self.root.as_ref().prev };

        unsafe {
            self.link_after(node, at);
        }

        self.len += 1;
        Element::new(node)
    }

    /// Caller must ensure `mark` is a live element currently linked in `self`.
    pub(crate) unsafe fn insert_before(&mut self, mark: Element<T>, value: T) -> Element<T> {
        debug_assert!(self.contains(mark));

        let node = self.alloc(value);
        let at = unsafe { mark.ptr.as_ref().prev };
        unsafe { self.link_after(node, at) };
        self.len += 1;
        Element::new(node)
    }

    /// Caller must ensure `mark` is a live element currently linked in `self`.
    pub(crate) unsafe fn insert_after(&mut self, mark: Element<T>, value: T) -> Element<T> {
        debug_assert!(self.contains(mark));

        let node = self.alloc(value);
        unsafe { self.link_after(node, mark.ptr) };
        self.len += 1;
        Element::new(node)
    }

    /// Caller must ensure `element` is a live element currently linked in `self`.
    pub(crate) unsafe fn remove(&mut self, element: Element<T>) -> T {
        debug_assert!(self.contains(element));

        unsafe { self.unlink(element.ptr) };
        self.len -= 1;

        let mut boxed = unsafe { Box::from_raw(element.ptr.as_ptr()) };
        boxed.root = None;
        boxed.prev = NonNull::dangling();
        boxed.next = NonNull::dangling();
        boxed.value.take().expect("non-root element must carry a value")
    }

    /// Caller must ensure `element` is a live element currently linked in `self`.
    pub(crate) unsafe fn move_to_front(&mut self, element: Element<T>) {
        debug_assert!(self.contains(element));

        if unsafe { self.root.as_ref().next } == element.ptr {
            return;
        }

        unsafe {
            self.unlink(element.ptr);
            self.link_after(element.ptr, self.root);
        }
    }

    /// Caller must ensure `element` is a live element currently linked in `self`.
    pub(crate) unsafe fn move_to_back(&mut self, element: Element<T>) {
        debug_assert!(self.contains(element));

        if unsafe { self.root.as_ref().prev } == element.ptr {
            return;
        }

        let at = unsafe { self.root.as_ref().prev };
        unsafe {
            self.unlink(element.ptr);
            self.link_after(element.ptr, at);
        }
    }

    /// Caller must ensure both handles are live elements currently linked in `self`.
    pub(crate) unsafe fn move_before(&mut self, element: Element<T>, mark: Element<T>) {
        debug_assert!(self.contains(element));
        debug_assert!(self.contains(mark));

        if element == mark || unsafe { element.ptr.as_ref().next } == mark.ptr {
            return;
        }

        let at = unsafe { mark.ptr.as_ref().prev };
        unsafe {
            self.unlink(element.ptr);
            self.link_after(element.ptr, at);
        }
    }

    /// Caller must ensure both handles are live elements currently linked in `self`.
    pub(crate) unsafe fn move_after(&mut self, element: Element<T>, mark: Element<T>) {
        debug_assert!(self.contains(element));
        debug_assert!(self.contains(mark));

        if element == mark || unsafe { element.ptr.as_ref().prev } == mark.ptr {
            return;
        }

        unsafe {
            self.unlink(element.ptr);
            self.link_after(element.ptr, mark.ptr);
        }
    }

    /// Caller must ensure `element` is a live element currently linked in `self`.
    pub(crate) unsafe fn next(&self, element: Element<T>) -> Option<Element<T>> {
        debug_assert!(self.contains(element));

        let next = unsafe { element.ptr.as_ref().next };
        if next == self.root {
            None
        } else {
            Some(Element::new(next))
        }
    }

    /// Caller must ensure `element` is a live element currently linked in `self`.
    pub(crate) unsafe fn prev(&self, element: Element<T>) -> Option<Element<T>> {
        debug_assert!(self.contains(element));

        let prev = unsafe { element.ptr.as_ref().prev };
        if prev == self.root {
            None
        } else {
            Some(Element::new(prev))
        }
    }

    /// Caller must ensure `element` is a live element currently linked in `self`.
    pub(crate) unsafe fn get(&self, element: Element<T>) -> &T {
        debug_assert!(self.contains(element));

        unsafe {
            element
                .ptr
                .as_ref()
                .value
                .as_ref()
                .expect("non-root element must carry a value")
        }
    }

    /// Caller must ensure `element` is a live element currently linked in `self`.
    pub(crate) unsafe fn get_mut(&mut self, element: Element<T>) -> &mut T {
        debug_assert!(self.contains(element));

        unsafe {
            (*element.ptr.as_ptr())
                .value
                .as_mut()
                .expect("non-root element must carry a value")
        }
    }

    pub(crate) fn iter(&self) -> Iter<'_, T> {
        Iter {
            list: self,
            current: self.front(),
        }
    }

    pub(crate) fn push_back_list(&mut self, other: &Self)
    where
        T: Clone,
    {
        let mut current = other.front();
        let mut remaining = other.len();

        while remaining > 0 {
            let element = current.expect("list length and traversal diverged");
            let next = unsafe { other.next(element) };
            let value = unsafe { other.get(element) }.clone();
            self.push_back(value);
            current = next;
            remaining -= 1;
        }
    }

    pub(crate) fn push_front_list(&mut self, other: &Self)
    where
        T: Clone,
    {
        let mut current = other.back();
        let mut remaining = other.len();

        while remaining > 0 {
            let element = current.expect("list length and traversal diverged");
            let prev = unsafe { other.prev(element) };
            let value = unsafe { other.get(element) }.clone();
            self.push_front(value);
            current = prev;
            remaining -= 1;
        }
    }

    fn alloc(&self, value: T) -> NonNull<Node<T>> {
        NonNull::new(Box::into_raw(Box::new(Node::new_detached(value))))
            .expect("Box::into_raw cannot return null")
    }

    fn contains(&self, element: Element<T>) -> bool {
        let node = unsafe { element.ptr.as_ref() };
        element.ptr != self.root && node.root == Some(self.root)
    }

    unsafe fn link_after(&mut self, element: NonNull<Node<T>>, at: NonNull<Node<T>>) {
        let next = unsafe { at.as_ref().next };

        unsafe {
            (*element.as_ptr()).prev = at;
            (*element.as_ptr()).next = next;
            (*element.as_ptr()).root = Some(self.root);
            (*at.as_ptr()).next = element;
            (*next.as_ptr()).prev = element;
        }
    }

    unsafe fn unlink(&mut self, element: NonNull<Node<T>>) {
        let prev = unsafe { element.as_ref().prev };
        let next = unsafe { element.as_ref().next };

        unsafe {
            (*prev.as_ptr()).next = next;
            (*next.as_ptr()).prev = prev;
            (*element.as_ptr()).prev = element;
            (*element.as_ptr()).next = element;
        }
    }
}

impl<T> Default for DoublyLinkedList<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Drop for DoublyLinkedList<T> {
    fn drop(&mut self) {
        self.clear();
        unsafe {
            drop(Box::from_raw(self.root.as_ptr()));
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for DoublyLinkedList<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter().map(|(_, value)| value)).finish()
    }
}

pub(crate) struct Iter<'a, T> {
    list: &'a DoublyLinkedList<T>,
    current: Option<Element<T>>,
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = (Element<T>, &'a T);

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.current?;
        let node = unsafe { current.ptr.as_ref() };
        let next = if node.next == self.list.root {
            None
        } else {
            Some(Element::new(node.next))
        };

        self.current = next;
        Some((
            current,
            node.value
                .as_ref()
                .expect("iterator must never yield the root element"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::DoublyLinkedList;

    fn assert_values<const N: usize>(list: &DoublyLinkedList<i32>, expected: [i32; N]) {
        assert!(list.iter().map(|(_, value)| *value).eq(expected));
    }

    #[test]
    fn push_front_and_back_preserve_order() {
        let mut list = DoublyLinkedList::new();

        list.push_back(2);
        list.push_front(1);
        list.push_back(3);

        assert_values(&list, [1, 2, 3]);
        assert_eq!(list.len(), 3);
    }

    #[test]
    fn insert_before_and_after_work() {
        let mut list = DoublyLinkedList::new();
        let one = list.push_back(1);
        let three = list.push_back(3);

        unsafe {
            let _two = list.insert_after(one, 2);
            let _zero = list.insert_before(one, 0);
            let _four = list.insert_after(three, 4);
        }

        assert_values(&list, [0, 1, 2, 3, 4]);
    }

    #[test]
    fn remove_updates_neighbors() {
        let mut list = DoublyLinkedList::new();
        let one = list.push_back(1);
        let two = list.push_back(2);
        let three = list.push_back(3);

        unsafe {
            assert_eq!(list.remove(two), 2);
            assert_eq!(list.next(one), Some(three));
            assert_eq!(list.prev(three), Some(one));
        }

        assert_values(&list, [1, 3]);
    }

    #[test]
    fn move_operations_reorder_existing_elements() {
        let mut list = DoublyLinkedList::new();
        let one = list.push_back(1);
        let two = list.push_back(2);
        let three = list.push_back(3);
        let four = list.push_back(4);

        unsafe {
            list.move_to_front(three);
            list.move_after(four, one);
            list.move_before(one, three);
            list.move_to_back(two);
        }

        assert_values(&list, [1, 3, 4, 2]);
    }

    #[test]
    fn clear_removes_all_elements() {
        let mut list = DoublyLinkedList::new();
        list.push_back(1);
        list.push_back(2);
        list.push_back(3);

        list.clear();

        assert!(list.is_empty());
        assert_eq!(list.front(), None);
        assert_eq!(list.back(), None);
    }

    #[test]
    fn push_back_list_and_push_front_list_copy_values() {
        let mut src = DoublyLinkedList::new();
        src.push_back(1);
        src.push_back(2);
        src.push_back(3);

        let mut dst = DoublyLinkedList::new();
        dst.push_back(10);
        dst.push_back_list(&src);
        assert_values(&dst, [10, 1, 2, 3]);

        let mut dst2 = DoublyLinkedList::new();
        dst2.push_back(10);
        dst2.push_front_list(&src);
        assert_values(&dst2, [1, 2, 3, 10]);
    }
}
