use crate::arena::Arena;
use crate::error::{Error, Result, Type};
use crate::object::{Cons, GcObject, IntoObject, LispFn, Object, Symbol, Value};
use crate::opcode::{CodeVec, OpCode};
use paste::paste;
use std::convert::TryInto;

impl OpCode {
    pub unsafe fn from_unchecked(x: u8) -> Self {
        std::mem::transmute(x)
    }
}

impl From<OpCode> for u8 {
    fn from(x: OpCode) -> u8 {
        x as u8
    }
}

impl Default for LispFn {
    fn default() -> Self {
        LispFn::new(
            vec_into![OpCode::Constant0, OpCode::Ret].into(),
            vec![Object::nil()],
            0,
            0,
            false,
        )
    }
}

#[derive(Debug)]
struct ConstVec {
    consts: Vec<GcObject>,
    arena: Arena,
}

impl<'obj> From<Vec<Object<'obj>>> for ConstVec {
    fn from(vec: Vec<Object<'obj>>) -> Self {
        let mut consts = ConstVec {
            consts: Vec::new(),
            arena: Arena::new(),
        };
        for x in vec.into_iter() {
            consts.insert_or_get(x);
        }
        consts
    }
}

impl PartialEq for ConstVec {
    fn eq(&self, other: &Self) -> bool {
        self.consts == other.consts
    }
}

impl ConstVec {
    pub const fn new() -> Self {
        ConstVec {
            consts: Vec::new(),
            arena: Arena::new(),
        }
    }

    fn insert_or_get(&mut self, obj: Object) -> usize {
        match self.consts.iter().position(|&x| obj == x) {
            None => {
                let new_obj = unsafe { obj.clone_in(&self.arena).into_gc() };
                self.consts.push(new_obj);
                self.consts.len() - 1
            }
            Some(x) => x,
        }
    }

    fn insert(&mut self, obj: Object) -> Result<u16> {
        let idx = self.insert_or_get(obj);
        match idx.try_into() {
            Ok(x) => Ok(x),
            Err(_) => Err(Error::ConstOverflow),
        }
    }

    fn insert_lambda(&mut self, func: LispFn) -> Result<u16> {
        let obj: Object = func.into_obj(&self.arena);
        self.consts.push(unsafe { obj.into_gc() });
        match (self.consts.len() - 1).try_into() {
            Ok(x) => Ok(x),
            Err(_) => Err(Error::ConstOverflow),
        }
    }
}

macro_rules! emit_op {
    ($self:ident, $op:ident, $idx:ident) => {
        match $idx {
            0 => $self.push_op(paste! {OpCode::[<$op 0>]}),
            1 => $self.push_op(paste! {OpCode::[<$op 1>]}),
            2 => $self.push_op(paste! {OpCode::[<$op 2>]}),
            3 => $self.push_op(paste! {OpCode::[<$op 3>]}),
            4 => $self.push_op(paste! {OpCode::[<$op 4>]}),
            5 => $self.push_op(paste! {OpCode::[<$op 5>]}),
            _ => match $idx.try_into() {
                Ok(n) => $self.push_op_n(paste! {OpCode::[<$op N>]}, n),
                Err(_) => $self.push_op_n2(paste! {OpCode::[<$op N2>]}, $idx),
            },
        }
    };
}

const JUMP_SLOTS: i16 = 2;

impl CodeVec {
    pub fn push_op(&mut self, op: OpCode) {
        self.push(op.into());
    }

    fn push_op_n(&mut self, op: OpCode, arg: u8) {
        self.push_op(op);
        self.push(arg);
    }

    fn push_op_n2(&mut self, op: OpCode, arg: u16) {
        self.push_op(op);
        self.push((arg >> 8) as u8);
        self.push(arg as u8);
    }

    fn push_jump_placeholder(&mut self) -> usize {
        let idx = self.len();
        self.push(0);
        self.push(0);
        idx
    }

    fn set_jump_placeholder(&mut self, index: usize) {
        let offset = self.len() as i16 - index as i16 - JUMP_SLOTS;
        self[index] = (offset >> 8) as u8;
        self[index + 1] = offset as u8;
    }

    fn push_back_jump(&mut self, index: usize) {
        let offset = index as i16 - self.len() as i16 - JUMP_SLOTS;
        self.push((offset >> 8) as u8);
        self.push(offset as u8);
    }

    fn emit_const(&mut self, idx: u16) {
        emit_op!(self, Constant, idx)
    }

    fn emit_varref(&mut self, idx: u16) {
        emit_op!(self, VarRef, idx)
    }

    fn emit_varset(&mut self, idx: u16) {
        emit_op!(self, VarSet, idx)
    }

    fn emit_call(&mut self, idx: u16) {
        emit_op!(self, Call, idx)
    }

    fn emit_stack_ref(&mut self, idx: u16) {
        emit_op!(self, StackRef, idx)
    }

    fn emit_stack_set(&mut self, idx: u16) {
        emit_op!(self, StackSet, idx)
    }
}

fn push_cons<'obj>(obj: Object<'obj>, mut vec: Vec<Object<'obj>>) -> Result<Vec<Object<'obj>>> {
    match obj.val() {
        Value::Nil => Ok(vec),
        Value::Cons(cons) => {
            vec.push(cons.car());
            push_cons(cons.cdr(), vec)
        }
        x => Err(Error::Type(Type::List, x.get_type())),
    }
}

fn into_list(obj: Object) -> Result<Vec<Object>> {
    push_cons(obj, vec![])
}

#[derive(Debug, PartialEq)]
pub struct Exp {
    codes: CodeVec,
    constants: ConstVec,
    vars: Vec<Option<Symbol>>,
}

impl From<Exp> for LispFn {
    fn from(exp: Exp) -> Self {
        let inner = exp.constants.consts;
        std::mem::forget(exp.constants.arena);
        LispFn::new(exp.codes, inner, 0, 0, false)
    }
}

impl<'obj> Exp {
    fn const_ref(&mut self, obj: Object<'obj>, var_ref: Option<Symbol>) -> Result<()> {
        self.vars.push(var_ref);
        let idx = self.constants.insert(obj)?;
        self.codes.emit_const(idx);
        Ok(())
    }

    fn add_const_lambda(&mut self, func: LispFn) -> Result<()> {
        self.vars.push(None);
        let idx = self.constants.insert_lambda(func)?;
        self.codes.emit_const(idx);
        Ok(())
    }

    fn stack_ref(&mut self, idx: usize, var_ref: Symbol) -> Result<()> {
        match (self.vars.len() - idx - 1).try_into() {
            Ok(x) => {
                self.vars.push(Some(var_ref));
                self.codes.emit_stack_ref(x);
                Ok(())
            }
            Err(_) => Err(Error::StackSizeOverflow),
        }
    }

    fn stack_set(&mut self, idx: usize) -> Result<()> {
        match (self.vars.len() - idx - 1).try_into() {
            Ok(x) => {
                self.vars.pop();
                self.codes.emit_stack_set(x);
                Ok(())
            }
            Err(_) => Err(Error::StackSizeOverflow),
        }
    }

    fn var_set(&mut self, idx: u16) {
        self.codes.emit_varset(idx);
        self.vars.pop();
    }

    fn discard(&mut self) {
        self.codes.push_op(OpCode::Discard);
        self.vars.pop();
    }

    fn duplicate(&mut self) {
        self.codes.push_op(OpCode::Duplicate);
        self.vars.push(None);
    }

    fn quote(&mut self, value: Object<'obj>) -> Result<()> {
        let list = into_list(value)?;
        match list.len() {
            // (quote x)
            1 => self.const_ref(list[0], None),
            // (quote) | (quote x y)
            x => Err(Error::ArgCount(1, x as u16)),
        }
    }

    fn compile_let(&mut self, form: Object) -> Result<()> {
        let list = into_list(form)?;
        let mut iter = list.into_iter();
        let num_binding_forms = match iter.next() {
            // (let x ...)
            Some(x) => self.let_bind(x)?,
            // (let)
            None => return Err(Error::ArgCount(1, 0)),
        };
        self.implicit_progn(iter.as_slice())?;
        // Remove let bindings from the stack
        if num_binding_forms > 0 {
            self.codes
                .push_op_n(OpCode::DiscardNKeepTOS, num_binding_forms as u8);
            let last = self.vars.pop().expect("empty stack in compile");
            self.vars.truncate(self.vars.len() - num_binding_forms);
            self.vars.push(last);
        }
        Ok(())
    }

    fn progn(&mut self, forms: Object) -> Result<()> {
        self.implicit_progn(into_list(forms)?.as_ref())
    }

    fn implicit_progn(&mut self, forms: &[Object]) -> Result<()> {
        if forms.is_empty() {
            self.const_ref(Object::nil(), None)
        } else {
            // Use take and skip to ensure that the last form does not discard
            for form in forms.iter().take(1) {
                self.compile_form(*form)?;
            }
            for form in forms.iter().skip(1) {
                self.discard();
                self.compile_form(*form)?;
            }
            Ok(())
        }
    }

    fn let_bind_call(&mut self, cons: &Cons) -> Result<()> {
        let var: Symbol = dbg!(cons.car().try_into())?;
        let list = into_list(cons.cdr())?;
        let len = list.len();
        let mut iter = list.into_iter();
        match iter.next() {
            // (let ((x y)))
            Some(value) => {
                self.compile_form(value)?;
                let last = self.vars.last_mut();
                let tos = last.expect("stack empty after compile form");
                *tos = Some(var);
            }
            // (let ((x)))
            None => self.const_ref(Object::nil(), Some(var))?,
        };
        match iter.next() {
            // (let ((x y)))
            None => Ok(()),
            // (let ((x y z ..)))
            Some(_) => Err(Error::LetValueCount(len as u16)),
        }
    }

    fn let_bind_nil(&mut self, sym: Symbol) -> Result<()> {
        self.const_ref(Object::nil(), Some(sym))
    }

    fn let_bind(&mut self, obj: Object) -> Result<usize> {
        let bindings = into_list(obj)?;
        for binding in &bindings {
            match binding.val() {
                // (let ((x y)))
                Value::Cons(cons) => self.let_bind_call(cons)?,
                // (let (x))
                Value::Symbol(sym) => self.let_bind_nil(sym)?,
                x => return Err(Error::Type(Type::Cons, x.get_type())),
            }
        }
        Ok(bindings.len())
    }

    fn setq(&mut self, obj: Object) -> Result<()> {
        let list = into_list(obj)?;
        let pairs = list.chunks_exact(2);
        let len = list.len() as u16;
        // (setq) | (setq x)
        if len < 2 {
            return Err(Error::ArgCount(2, len));
        }
        // (setq x y z)
        if len % 2 != 0 {
            // We have an extra element in the setq that does not have a value
            return Err(Error::ArgCount(len + 1, len));
        }
        let last = (list.len() / 2) - 1;
        for (idx, pair) in pairs.enumerate() {
            let sym: Symbol = pair[0].try_into()?;
            let val = pair[1];

            self.compile_form(val)?;

            // Duplicate the last value to be the return value of the setq
            // expression
            if idx == last {
                self.duplicate();
            }

            match self.vars.iter().rposition(|&x| x == Some(sym)) {
                Some(idx) => self.stack_set(idx)?,
                None => {
                    let idx = self.constants.insert(sym.into())?;
                    self.var_set(idx);
                }
            };
        }
        Ok(())
    }

    fn compile_funcall(&mut self, cons: &Cons) -> Result<()> {
        self.const_ref(cons.car(), None)?;
        let prev_len = self.vars.len();
        let args = into_list(cons.cdr())?;
        let num_args = args.len();
        for arg in args {
            self.compile_form(arg)?;
        }
        self.codes.emit_call(num_args as u16);
        self.vars.truncate(prev_len);
        Ok(())
    }

    fn jump(&mut self, jump_code: OpCode) -> (usize, OpCode) {
        match jump_code {
            OpCode::JumpNil
            | OpCode::JumpNotNil
            | OpCode::JumpNilElsePop
            | OpCode::JumpNotNilElsePop => {
                self.vars.pop();
            }
            OpCode::Jump => {}
            x => panic!("invalid jump opcode provided: {:?}", x),
        }
        self.codes.push_op(jump_code);
        let place = self.codes.push_jump_placeholder();
        (place, jump_code)
    }

    fn set_jump_target(&mut self, target: (usize, OpCode)) {
        match target.1 {
            // add the non-popped conditional back to the stack, since we are
            // past the "else pop" part of the Code
            OpCode::JumpNilElsePop | OpCode::JumpNotNilElsePop => {
                self.vars.push(None);
            }
            OpCode::JumpNil | OpCode::JumpNotNil | OpCode::Jump => {}
            x => panic!("invalid jump opcode provided: {:?}", x),
        }
        self.codes.set_jump_placeholder(target.0);
    }

    fn jump_back(&mut self, jump_code: OpCode, location: usize) {
        if matches!(jump_code, OpCode::Jump) {
            self.codes.push_op(OpCode::Jump);
            self.codes.push_back_jump(location);
        } else {
            panic!("invalid back jump opcode provided: {:?}", jump_code)
        }
    }

    fn compile_if(&mut self, obj: Object) -> Result<()> {
        let list = into_list(obj)?;
        match list.len() {
            // (if) | (if x)
            len @ 0 | len @ 1 => Err(Error::ArgCount(2, len as u16)),
            // (if x y)
            2 => {
                self.compile_form(list[0])?;
                let target = self.jump(OpCode::JumpNilElsePop);
                self.compile_form(list[1])?;
                self.set_jump_target(target);
                Ok(())
            }
            // (if x y z ...)
            _ => {
                let mut forms = list.into_iter();
                self.compile_form(forms.next().unwrap())?;
                let else_nil_target = self.jump(OpCode::JumpNil);
                // if branch
                self.compile_form(forms.next().unwrap())?;
                let jump_to_end_target = self.jump(OpCode::Jump);
                // else branch
                self.set_jump_target(else_nil_target);
                self.implicit_progn(forms.as_slice())?;
                self.set_jump_target(jump_to_end_target);
                Ok(())
            }
        }
    }

    fn compile_loop(&mut self, obj: Object) -> Result<()> {
        let forms = into_list(obj)?;
        if forms.is_empty() {
            return Err(Error::ArgCount(1, 0));
        }
        let top = self.codes.len();
        self.compile_form(forms[0])?;
        let loop_exit = self.jump(OpCode::JumpNilElsePop);
        self.implicit_progn(&forms[1..])?;
        self.discard();
        self.jump_back(OpCode::Jump, top);
        self.set_jump_target(loop_exit);
        Ok(())
    }

    fn compile_lambda(&mut self, obj: Object) -> Result<()> {
        let list = into_list(obj)?;
        let mut iter = list.into_iter();
        let mut vars: Vec<Option<Symbol>> = vec![];

        match iter.next() {
            // (lambda ())
            None => {
                return self.add_const_lambda(LispFn::default());
            }
            // (lambda (x ...) ...)
            Some(bindings) => {
                for binding in &into_list(bindings)? {
                    println!("binding = {}", binding);
                    match binding.val() {
                        Value::Symbol(x) => vars.push(Some(x)),
                        x => return dbg!(Err(Error::Type(Type::Symbol, x.get_type()))),
                    }
                }
            }
        };
        let body = iter.as_slice();
        if body.is_empty() {
            self.add_const_lambda(LispFn::default())
        } else {
            let len = vars.len();
            let mut func: LispFn = Self::compile_func_body(body, vars)?.into();
            func.args.required = len as u16;
            self.add_const_lambda(func)
        }
    }

    fn compile_defvar(&mut self, obj: Object) -> Result<()> {
        let list = into_list(obj)?;
        let mut iter = list.into_iter();

        match iter.next() {
            // (defvar x ...)
            Some(x) => {
                match x.val() {
                    Value::Symbol(sym) => {
                        // TODO: compile this into a lambda like Emacs does
                        match iter.next() {
                            // (defvar x y)
                            Some(value) => self.compile_form(value)?,
                            // (defvar x)
                            None => self.const_ref(Object::nil(), None)?,
                        };
                        self.duplicate();
                        let idx = self.constants.insert(sym.into())?;
                        self.var_set(idx);
                        Ok(())
                    }
                    // (defvar "x")
                    x => Err(Error::Type(Type::Symbol, x.get_type())),
                }
            }
            // (defvar)
            None => Err(Error::ArgCount(1, 0)),
        }
    }

    fn compile_cond_clause(
        &mut self,
        clause: Object,
        jump_targets: &mut Vec<(usize, OpCode)>,
    ) -> Result<()> {
        let cond = into_list(clause)?;
        match cond.len() {
            // (cond ())
            0 => {}
            // (cond (x))
            1 => {
                self.compile_form(cond[0])?;
                let target = self.jump(OpCode::JumpNotNilElsePop);
                jump_targets.push(target);
            }
            // (cond (x y ...))
            _ => {
                self.compile_form(cond[0])?;
                let skip_target = self.jump(OpCode::JumpNil);
                self.implicit_progn(&cond[1..])?;
                self.vars.pop();
                let taken_target = self.jump(OpCode::Jump);
                self.set_jump_target(skip_target);
                jump_targets.push(taken_target);
            }
        }
        Ok(())
    }

    fn compile_last_cond_clause(
        &mut self,
        clause: Object,
        jump_targets: &mut Vec<(usize, OpCode)>,
    ) -> Result<()> {
        let cond = into_list(clause)?;
        match cond.len() {
            // (cond ())
            0 => {
                self.const_ref(Object::nil(), None)?;
            }
            // (cond (x))
            1 => {
                self.compile_form(cond[0])?;
                let target = self.jump(OpCode::JumpNotNilElsePop);
                self.const_ref(Object::nil(), None)?;
                jump_targets.push(target);
            }
            // (cond (x y ...))
            _ => {
                self.compile_form(cond[0])?;
                let target = self.jump(OpCode::JumpNilElsePop);
                self.implicit_progn(&cond[1..])?;
                jump_targets.push(target);
            }
        }
        Ok(())
    }

    fn compile_cond(&mut self, obj: Object) -> Result<()> {
        let mut clauses = into_list(obj)?;
        let last = match clauses.pop() {
            Some(clause) => clause,
            // (cond)
            None => {
                return self.const_ref(Object::nil(), None);
            }
        };
        let final_return_targets = &mut Vec::new();
        for clause in clauses {
            self.compile_cond_clause(clause, final_return_targets)?;
        }
        self.compile_last_cond_clause(last, final_return_targets)?;

        for target in final_return_targets {
            self.codes.set_jump_placeholder(target.0);
        }
        Ok(())
    }

    fn dispatch_special_form(&mut self, cons: &Cons) -> Result<()> {
        println!("car = {}", cons.car());
        let sym: Symbol = dbg!(cons.car().try_into())?;
        match sym.get_name() {
            "lambda" => self.compile_lambda(cons.cdr()),
            "while" => self.compile_loop(cons.cdr()),
            "quote" => self.quote(cons.cdr()),
            "progn" => self.progn(cons.cdr()),
            "setq" => self.setq(cons.cdr()),
            "defvar" => self.compile_defvar(cons.cdr()),
            "cond" => self.compile_cond(cons.cdr()),
            "let" => self.compile_let(cons.cdr()),
            "if" => self.compile_if(cons.cdr()),
            _ => self.compile_funcall(cons),
        }
    }

    fn variable_reference(&mut self, sym: Symbol) -> Result<()> {
        match self.vars.iter().rposition(|&x| x == Some(sym)) {
            Some(idx) => self.stack_ref(idx, sym),
            None => {
                let idx = self.constants.insert(sym.into())?;
                self.codes.emit_varref(idx);
                self.vars.push(None);
                Ok(())
            }
        }
    }

    fn compile_form(&mut self, obj: Object<'obj>) -> Result<()> {
        match dbg!(obj.val()) {
            Value::Cons(cons) => self.dispatch_special_form(cons),
            Value::Symbol(sym) => self.variable_reference(sym),
            _ => self.const_ref(obj, None),
        }
    }

    fn compile_func_body(obj: &[Object], vars: Vec<Option<Symbol>>) -> Result<Self> {
        let mut exp = Self {
            codes: CodeVec::default(),
            constants: ConstVec::new(),
            vars,
        };
        exp.implicit_progn(obj)?;
        exp.codes.push_op(OpCode::Ret);
        exp.vars.truncate(0);
        Ok(exp)
    }

    pub fn compile(obj: Object) -> Result<Self> {
        Self::compile_func_body(&[obj], vec![])
    }
}

#[cfg(test)]
mod test {

    use super::*;
    use crate::arena::Arena;
    use crate::intern::intern;
    use crate::reader::Reader;
    use OpCode::*;

    fn check_error(compare: &str, expect: Error) {
        let arena = &Arena::new();
        let obj = Reader::read(compare, arena).unwrap().0;
        assert_eq!(Exp::compile(obj).err().unwrap(), expect);
    }

    macro_rules! check_compiler {
        ($compare:expr, [$($op:expr),+], [$($const:expr),+]) => {
            let arena = &Arena::new();
            println!("Test String: {}", $compare);
            let obj = Reader::read($compare, arena).unwrap().0;
            let expect = Exp{
                codes:vec_into![$($op),+].into(),
                constants: ConstVec::from(vec_into_object![$($const),+; arena]),
                vars: Vec::new(),
            };
            assert_eq!(Exp::compile(obj).unwrap(), expect);
        }
    }

    #[test]
    fn test_basic() {
        let arena = &Arena::new();
        check_compiler!("1", [Constant0, Ret], [1]);
        check_compiler!("'foo", [Constant0, Ret], [intern("foo")]);
        check_compiler!("'(1 2)", [Constant0, Ret], [list!(1, 2; arena)]);
        check_compiler!("\"foo\"", [Constant0, Ret], ["foo"]);
    }

    #[test]
    fn test_compile_variable() {
        check_compiler!(
            "(let (foo))",
            [Constant0, Constant0, DiscardNKeepTOS, 1, Ret],
            [false]
        );
        check_compiler!("(let ())", [Constant0, Ret], [false]);
        check_compiler!(
            "(let ((foo 1)(bar 2)(baz 3)))",
            [
                Constant0,
                Constant1,
                Constant2,
                Constant3,
                DiscardNKeepTOS,
                3,
                Ret
            ],
            [1, 2, 3, false]
        );
        check_compiler!(
            "(let ((foo 1)) foo)",
            [Constant0, StackRef0, DiscardNKeepTOS, 1, Ret],
            [1]
        );
        check_compiler!("foo", [VarRef0, Ret], [intern("foo")]);
        check_compiler!("(progn)", [Constant0, Ret], [false]);
        check_compiler!(
            "(progn (set 'foo 5) foo)",
            [Constant0, Constant1, Constant2, Call2, Discard, VarRef1, Ret],
            [intern("set"), intern("foo"), 5]
        );
        check_compiler!(
            "(let ((foo 1)) (setq foo 2) foo)",
            [
                Constant0,
                Constant1,
                Duplicate,
                StackSet2,
                Discard,
                StackRef0,
                DiscardNKeepTOS,
                1,
                Ret
            ],
            [1, 2]
        );
        check_compiler!(
            "(progn (setq foo 2) foo)",
            [Constant0, Duplicate, VarSet1, Discard, VarRef1, Ret],
            [2, intern("foo")]
        );
        check_compiler!(
            "(let ((bar 4)) (+ foo bar))",
            [
                Constant0,
                Constant1,
                VarRef2,
                StackRef2,
                Call2,
                DiscardNKeepTOS,
                1,
                Ret
            ],
            [4, intern("+"), intern("foo")]
        );
        check_compiler!(
            "(defvar foo 1)",
            [Constant0, Duplicate, VarSet1, Ret],
            [1, intern("foo")]
        );
        check_compiler!(
            "(defvar foo)",
            [Constant0, Duplicate, VarSet1, Ret],
            [false, intern("foo")]
        );
        check_error("(let (foo 1))", Error::Type(Type::Cons, Type::Int));
    }

    const fn get_jump_slots(offset: i16) -> (u8, u8) {
        ((offset >> 8) as u8, offset as u8)
    }

    #[test]
    fn conditional() {
        let (high4, low4) = get_jump_slots(4);
        let (high1, low1) = get_jump_slots(1);
        check_compiler!(
            "(if nil 1 2)",
            [Constant0, JumpNil, high4, low4, Constant1, Jump, high1, low1, Constant2, Ret],
            [Object::nil(), 1, 2]
        );
        check_compiler!(
            "(if t 2)",
            [Constant0, JumpNilElsePop, high1, low1, Constant1, Ret],
            [Object::t(), 2]
        );
        check_error("(if 1)", Error::ArgCount(2, 1));
    }

    #[test]
    fn cond_stmt() {
        check_compiler!("(cond)", [Constant0, Ret], [Object::nil()]);
        check_compiler!("(cond ())", [Constant0, Ret], [Object::nil()]);
        check_compiler!(
            "(cond (1))",
            [Constant0, JumpNotNilElsePop, 0, 1, Constant1, Ret],
            [1, false]
        );
        check_compiler!(
            "(cond (1 2))",
            [Constant0, JumpNilElsePop, 0, 1, Constant1, Ret],
            [1, 2]
        );
        check_compiler!(
            "(cond (1 2)(3 4))",
            [
                Constant0,
                JumpNil,
                0,
                4,
                Constant1,
                Jump,
                0,
                5,
                Constant2,
                JumpNilElsePop,
                0,
                1,
                Constant3,
                Ret
            ],
            [1, 2, 3, 4]
        );
        check_compiler!(
            "(cond (1)(2))",
            [
                Constant0,
                JumpNotNilElsePop,
                0,
                5,
                Constant1,
                JumpNotNilElsePop,
                0,
                1,
                Constant2,
                Ret
            ],
            [1, 2, false]
        );
    }

    #[test]
    fn while_loop() {
        let (high5, low5) = get_jump_slots(5);
        let (high_9, low_9) = get_jump_slots(-9);
        check_compiler!(
            "(while t)",
            [
                Constant0,
                JumpNilElsePop,
                high5,
                low5,
                Constant1,
                Discard,
                Jump,
                high_9,
                low_9,
                Ret
            ],
            [Object::t(), Object::nil()]
        );

        check_compiler!(
            "(while t 1)",
            [
                Constant0,
                JumpNilElsePop,
                high5,
                low5,
                Constant1,
                Discard,
                Jump,
                high_9,
                low_9,
                Ret
            ],
            [Object::t(), 1]
        );

        check_compiler!(
            "(while nil 2)",
            [
                Constant0,
                JumpNilElsePop,
                high5,
                low5,
                Constant1,
                Discard,
                Jump,
                high_9,
                low_9,
                Ret
            ],
            [Object::nil(), 2]
        );

        let (high7, low7) = get_jump_slots(7);
        let (high_11, low_11) = get_jump_slots(-11);
        check_compiler!(
            "(while nil 2 3)",
            [
                Constant0,
                JumpNilElsePop,
                high7,
                low7,
                Constant1,
                Discard,
                Constant2,
                Discard,
                Jump,
                high_11,
                low_11,
                Ret
            ],
            [Object::nil(), 2, 3]
        );
        check_error("(while)", Error::ArgCount(1, 0));
    }

    #[test]
    fn function() {
        check_compiler!("(foo)", [Constant0, Call0, Ret], [intern("foo")]);
        check_compiler!(
            "(foo 1 2)",
            [Constant0, Constant1, Constant2, Call2, Ret],
            [intern("foo"), 1, 2]
        );
        check_compiler!(
            "(foo (bar 1) 2)",
            [Constant0, Constant1, Constant2, Call1, Constant3, Call2, Ret],
            [intern("foo"), intern("bar"), 1, 2]
        );
        check_compiler!(
            "(foo (bar 1) (baz 1))",
            [Constant0, Constant1, Constant2, Call1, Constant3, Constant2, Call1, Call2, Ret],
            [intern("foo"), intern("bar"), 1, intern("baz")]
        );
        check_error("(foo . 1)", Error::Type(Type::List, Type::Int));
    }

    #[test]
    fn lambda() {
        let arena = &Arena::new();
        check_compiler!("(lambda)", [Constant0, Ret], [LispFn::default()]);
        check_compiler!("(lambda ())", [Constant0, Ret], [LispFn::default()]);
        check_compiler!("(lambda () nil)", [Constant0, Ret], [LispFn::default()]);

        let constant: Object = 1.into_obj(arena);
        let func = LispFn::new(
            vec_into![Constant0, Ret].into(),
            vec![unsafe { constant.into_gc() }],
            0,
            0,
            false,
        );
        check_compiler!("(lambda () 1)", [Constant0, Ret], [func]);

        let func = LispFn::new(vec_into![StackRef0, Ret].into(), vec![], 1, 0, false);
        check_compiler!("(lambda (x) x)", [Constant0, Ret], [func]);

        let func = LispFn::new(
            vec_into![Constant0, StackRef2, StackRef2, Call2, Ret].into(),
            vec_into![intern("+")],
            2,
            0,
            false,
        );
        check_compiler!("(lambda (x y) (+ x y))", [Constant0, Ret], [func]);

        check_error("(lambda (x 1) x)", Error::Type(Type::Symbol, Type::Int));
    }

    #[test]
    fn errors() {
        check_error("(\"foo\")", Error::Type(Type::Symbol, Type::String));
        check_error("(quote)", Error::ArgCount(1, 0));
        check_error("(quote 1 2)", Error::ArgCount(1, 2))
    }

    #[test]
    fn let_errors() {
        check_error("(let (1))", Error::Type(Type::Cons, Type::Int));
        check_error("(let ((foo 1 2)))", Error::LetValueCount(2));
        check_error("(let ((foo . 1)))", Error::Type(Type::List, Type::Int));
        check_error("(let ((foo 1 . 2)))", Error::Type(Type::List, Type::Int));
        check_error("(let (()))", Error::Type(Type::Cons, Type::Nil));
        check_error("(let)", Error::ArgCount(1, 0));
    }
}
