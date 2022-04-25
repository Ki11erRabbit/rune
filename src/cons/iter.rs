use crate::{
    arena::RootOwner,
    arena::{Arena, ConstrainLifetime, Root, RootCons, RootObj, RootRef},
    error::{Error, Type},
    object::{Gc, ListX, Object, ObjectX},
};
use streaming_iterator::StreamingIterator;

use anyhow::{anyhow, Result};

use super::Cons;

#[derive(Clone)]
pub(crate) struct ElemIter<'ob> {
    cons: Option<&'ob Cons>,
    arena: &'ob Arena<'ob>,
}

impl<'brw> Cons {
    pub(crate) fn elements<'new>(&'brw self, arena: &'new Arena) -> ElemIter<'new> {
        ElemIter {
            cons: Some(self.constrain_lifetime(arena)),
            arena,
        }
    }
}

impl<'ob> Gc<ListX<'ob>> {
    pub(crate) fn elements(self, arena: &'ob Arena) -> ElemIter<'ob> {
        match self.get() {
            ListX::Nil => ElemIter { cons: None, arena },
            ListX::Cons(cons) => ElemIter {
                cons: Some(cons.constrain_lifetime(arena)),
                arena,
            },
        }
    }

    pub(crate) fn conses(self, arena: &'ob Arena) -> ConsIter<'ob> {
        ConsIter {
            list: self.get(),
            arena,
        }
    }
}

impl<'ob> Iterator for ElemIter<'ob> {
    type Item = Result<Object<'ob>>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.cons {
            Some(cons) => {
                (*self).cons = match cons.cdr(self.arena).get() {
                    ObjectX::Cons(next) => Some(next),
                    ObjectX::Nil => None,
                    _ => return Some(Err(anyhow!("Found non-nil cdr at end of list"))),
                };
                Some(Ok(cons.car(self.arena)))
            }
            None => None,
        }
    }
}

impl<'ob> Object<'ob> {
    pub(crate) fn as_list<'gc>(self, arena: &'gc Arena) -> Result<ElemIter<'gc>> {
        match self.get() {
            ObjectX::Cons(cons) => {
                let cons = cons.constrain_lifetime(arena);
                Ok(ElemIter {
                    cons: Some(cons),
                    arena,
                })
            }
            ObjectX::Nil => Ok(ElemIter { cons: None, arena }),
            _ => Err(Error::from_object(Type::List, self).into()),
        }
    }
}

impl<'ob> ElemIter<'ob> {
    pub(crate) fn len(&self) -> usize {
        self.clone().count()
    }
}

#[derive(Clone)]
pub(crate) struct ConsIter<'ob> {
    list: ListX<'ob>,
    arena: &'ob Arena<'ob>,
}

impl<'ob> Iterator for ConsIter<'ob> {
    type Item = Result<&'ob Cons>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.list {
            ListX::Nil => None,
            ListX::Cons(cons) => {
                self.list = match cons.cdr(self.arena).get() {
                    ObjectX::Cons(next) => ListX::Cons(next),
                    ObjectX::Nil => ListX::Nil,
                    _ => return Some(Err(anyhow::anyhow!("Found non-nil cdr at end of list"))),
                };
                Some(Ok(cons))
            }
        }
    }
}

pub(crate) struct ElemStreamIter<'rt, 'id> {
    elem: Option<&'rt Root<'id, RootObj>>,
    cons: Option<&'rt Root<'id, RootCons>>,
    owner: RootOwner<'id>,
}

impl<'rt, 'id> ElemStreamIter<'rt, 'id> {
    pub(crate) fn new(
        elem: &'rt Option<Root<'id, RootObj>>,
        cons: &'rt Option<Root<'id, RootCons>>,
        owner: RootOwner<'id>,
    ) -> Self {
        Self {
            elem: elem.as_ref(),
            cons: cons.as_ref(),
            owner,
        }
    }
}

impl<'rt, 'id> StreamingIterator for ElemStreamIter<'rt, 'id> {
    type Item = RootRef<RootObj>;

    fn advance(&mut self) {
        if let Some(cons) = &self.cons {
            let elem = self
                .elem
                .as_ref()
                .expect("Element should never be None while Cons is Some");
            let (cons, elem) = unsafe { Root::borrow_mut_unchecked2(cons, elem, &mut self.owner) };
            let car = cons.__car();
            elem.set(car);
            match cons.__cdr().get() {
                ObjectX::Cons(next) => {
                    let x = unsafe { std::mem::transmute::<&Cons, &Cons>(next) };
                    cons.set(x);
                }
                _ => {
                    self.cons = None;
                }
            }
        } else {
            self.elem = None;
        }
    }

    fn get(&self) -> Option<&Self::Item> {
        self.elem.as_ref().map(|x| x.borrow(&self.owner))
    }
}

impl<'rt, 'id> ElemStreamIter<'rt, 'id> {
    pub(crate) fn is_empty(&self) -> bool {
        self.cons.is_none()
    }
}

#[macro_export]
macro_rules! element_iter {
    ($ident:ident, $obj:expr, $gc:ident) => {
        let mut root_elem = None;
        let mut root_cons = None;
        generativity::make_guard!(guard);
        let owner = RootOwner::new(guard);

        let mut gc_root_elem = unsafe { $crate::arena::RootStruct::new($gc.get_root_set()) };
        let mut gc_root_cons = unsafe { $crate::arena::RootStruct::new($gc.get_root_set()) };
        #[allow(unused_qualifications)]
        let list: $crate::object::Gc<$crate::object::ListX> = $obj.try_into()?;
        if let $crate::object::ListX::Cons(cons) = list.get() {
            root_elem =
                unsafe { Some($crate::arena::Root::new($crate::arena::RootObj::default())) };
            root_cons =
                unsafe { Some($crate::arena::Root::new($crate::arena::RootCons::new(cons))) };
            gc_root_elem.set(root_elem.as_mut().unwrap());
            gc_root_cons.set(root_cons.as_mut().unwrap());
        } else {
            std::mem::forget(gc_root_elem);
            std::mem::forget(gc_root_cons);
        }
        #[allow(unused_mut)]
        let mut $ident = $crate::cons::ElemStreamIter::new(&root_elem, &root_cons, owner);
    };
}

#[cfg(test)]
mod test {
    use crate::{arena::RootSet, list};

    use super::*;

    #[test]
    fn elem_iter() {
        let roots = &RootSet::default();
        let arena = &Arena::new(roots);
        let cons = list![1, 2, 3, 4; arena];
        let iter = cons.as_list(arena).unwrap();
        let vec: Result<Vec<_>> = iter.collect();
        assert_eq!(vec.unwrap(), vec![1, 2, 3, 4]);
    }

    #[test]
    fn cons_iter() {
        let roots = &RootSet::default();
        let arena = &Arena::new(roots);
        let cons: Object = list![1, 2, 3, 4; arena];
        let list: Gc<ListX> = cons.try_into().unwrap();
        let iter = list.conses(arena);
        let expects = vec![1, 2, 3, 4];
        for (act, expect) in iter.zip(expects.into_iter()) {
            let actual = act.unwrap().car(arena);
            assert_eq!(actual, expect);
        }
    }

    #[test]
    fn stream_iter() {
        let func = || -> Result<()> {
            let roots = &RootSet::default();
            let arena = &Arena::new(roots);
            let cons: Object = list![1, 2, 3, 4; arena];
            element_iter!(iter, cons, arena);
            for expect in 1..=4 {
                let actual = iter.next().unwrap().obj();
                assert_eq!(actual, expect);
            }
            assert!(iter.is_empty());
            Ok(())
        };
        func().unwrap();
    }
}
