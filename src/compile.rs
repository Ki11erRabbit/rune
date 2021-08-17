use crate::arena::Arena;
use crate::cons::{Cons, ConsIter};
use crate::data::Environment;
use crate::error::{Error, Type};
use crate::eval;
use crate::object::{Expression, IntoObject, LispFn, Object, Value};
use crate::opcode::{CodeVec, OpCode};
use crate::symbol::{intern, Symbol};
use anyhow::{bail, ensure, Result};
use paste::paste;
use std::convert::TryInto;
use std::fmt::Display;

#[derive(Debug, PartialEq)]
pub(crate) enum CompError {
    ConstOverflow,
    LetValueCount(usize),
    StackSizeOverflow,
    RecursiveMacro,
}

impl Display for CompError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompError::ConstOverflow => write!(f, "Too many constants declared in fuction"),
            CompError::LetValueCount(_) => write!(f, "Let forms can only have 1 value"),
            CompError::StackSizeOverflow => write!(f, "Stack size overflow"),
            CompError::RecursiveMacro => write!(f, "Recursive macros are not supported"),
        }
    }
}

impl std::error::Error for CompError {}

#[derive(Debug)]
struct ConstVec<'ob> {
    consts: Vec<Object<'ob>>,
}

impl<'ob> From<Vec<Object<'ob>>> for ConstVec<'ob> {
    fn from(vec: Vec<Object<'ob>>) -> Self {
        let mut consts = ConstVec { consts: Vec::new() };
        for x in vec {
            consts.insert_or_get(x);
        }
        consts
    }
}

impl<'ob> PartialEq for ConstVec<'ob> {
    fn eq(&self, other: &Self) -> bool {
        self.consts == other.consts
    }
}

impl<'ob> ConstVec<'ob> {
    pub(crate) const fn new() -> Self {
        ConstVec { consts: Vec::new() }
    }

    fn insert_or_get(&mut self, obj: Object<'ob>) -> usize {
        match self.consts.iter().position(|&x| obj == x) {
            None => {
                self.consts.push(obj);
                self.consts.len() - 1
            }
            Some(x) => x,
        }
    }

    fn insert(&mut self, obj: Object<'ob>) -> Result<u16, CompError> {
        let idx = self.insert_or_get(obj);
        match idx.try_into() {
            Ok(x) => Ok(x),
            Err(_) => Err(CompError::ConstOverflow),
        }
    }

    fn insert_lambda(&mut self, func: LispFn<'ob>, arena: &'ob Arena) -> Result<u16, CompError> {
        let obj: Object = func.into_obj(arena);
        self.consts.push(obj);
        match (self.consts.len() - 1).try_into() {
            Ok(x) => Ok(x),
            Err(_) => Err(CompError::ConstOverflow),
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
    pub(crate) fn push_op(&mut self, op: OpCode) {
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
        emit_op!(self, Constant, idx);
    }

    fn emit_varref(&mut self, idx: u16) {
        emit_op!(self, VarRef, idx);
    }

    fn emit_varset(&mut self, idx: u16) {
        emit_op!(self, VarSet, idx);
    }

    fn emit_call(&mut self, idx: u16) {
        emit_op!(self, Call, idx);
    }

    fn emit_stack_ref(&mut self, idx: u16) {
        emit_op!(self, StackRef, idx);
    }

    fn emit_stack_set(&mut self, idx: u16) {
        emit_op!(self, StackSet, idx);
    }
}

fn into_iter(obj: Object) -> Result<ConsIter> {
    match obj.val() {
        Value::Cons(cons) => Ok(cons.into_iter()),
        Value::Nil => Ok(ConsIter::empty()),
        _ => Err(Error::from_object(Type::List, obj).into()),
    }
}

#[derive(Debug, PartialEq)]
struct Compiler<'ob, 'brw> {
    codes: CodeVec,
    constants: ConstVec<'ob>,
    vars: Vec<Option<Symbol>>,
    env: &'brw mut Environment<'ob>,
    arena: &'ob Arena,
}

impl<'ob> From<Compiler<'ob, '_>> for Expression<'ob> {
    fn from(exp: Compiler<'ob, '_>) -> Self {
        Self {
            constants: exp.constants.consts,
            op_codes: exp.codes,
        }
    }
}

impl<'ob, 'brw> Compiler<'ob, 'brw> {
    fn new(
        vars: Vec<Option<Symbol>>,
        env: &'brw mut Environment<'ob>,
        arena: &'ob Arena,
    ) -> Compiler<'ob, 'brw> {
        Self {
            codes: CodeVec::default(),
            constants: ConstVec::new(),
            env,
            vars,
            arena,
        }
    }

    fn const_ref(&mut self, obj: Object<'ob>, var_ref: Option<Symbol>) -> Result<()> {
        self.vars.push(var_ref);
        let idx = self.constants.insert(obj)?;
        self.codes.emit_const(idx);
        Ok(())
    }

    fn add_const_lambda(&mut self, func: LispFn<'ob>) -> Result<()> {
        self.vars.push(None);
        let idx = self.constants.insert_lambda(func, self.arena)?;
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
            Err(_) => Err(CompError::StackSizeOverflow.into()),
        }
    }

    fn stack_set(&mut self, idx: usize) -> Result<(), CompError> {
        match (self.vars.len() - idx - 1).try_into() {
            Ok(x) => {
                self.vars.pop();
                self.codes.emit_stack_set(x);
                Ok(())
            }
            Err(_) => Err(CompError::StackSizeOverflow),
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

    fn quote(&mut self, value: Object<'ob>) -> Result<()> {
        let mut forms = into_iter(value)?;
        let len = forms.clone().count();
        match len {
            1 => self.const_ref(forms.next().unwrap()?, None),
            x => Err(Error::ArgCount(1, x as u16).into()),
        }
    }

    fn compile_let(&mut self, form: Object<'ob>, parallel: bool) -> Result<()> {
        let mut iter = into_iter(form)?;
        let num_binding_forms = match iter.next() {
            // (let x ...)
            Some(x) => self.let_bind(x?, parallel)?,
            // (let)
            None => bail!(Error::ArgCount(1, 0)),
        };
        self.implicit_progn(iter)?;
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

    fn progn(&mut self, forms: Object<'ob>) -> Result<()> {
        self.implicit_progn(into_iter(forms)?)
    }

    fn implicit_progn(&mut self, mut forms: ConsIter<'_, 'ob>) -> Result<()> {
        if let Some(form) = forms.next() {
            self.compile_form(form?)?;
        } else {
            return self.const_ref(Object::Nil, None);
        }
        for form in forms {
            self.discard();
            self.compile_form(form?)?;
        }
        Ok(())
    }

    fn let_bind_call(&mut self, cons: &'ob Cons<'ob>) -> Result<Symbol> {
        let mut iter = into_iter(cons.cdr())?;
        match iter.next() {
            // (let ((x y)))
            Some(value) => self.compile_form(value?)?,
            // (let ((x)))
            None => self.const_ref(Object::Nil, None)?,
        };
        let rest = iter.count();
        // (let ((x y z ..)))
        ensure!(rest == 0, CompError::LetValueCount(rest + 1));
        Ok(cons.car().try_into()?)
    }

    fn let_bind_nil(&mut self, sym: Symbol) -> Result<()> {
        self.const_ref(Object::Nil, Some(sym))
    }

    fn let_bind(&mut self, obj: Object<'ob>, parallel: bool) -> Result<usize> {
        let bindings = into_iter(obj)?;
        let mut len = 0;
        let mut let_bindings = Vec::new();
        for binding in bindings {
            let binding = binding?;
            match binding.val() {
                // (let ((x y)))
                Value::Cons(cons) => {
                    let let_bound_var = self.let_bind_call(cons)?;
                    if parallel {
                        let_bindings.push(Some(let_bound_var));
                    } else {
                        let last = self.vars.last_mut();
                        let tos = last.expect("stack empty after compile form");
                        *tos = Some(let_bound_var);
                    }
                }
                // (let (x))
                Value::Symbol(sym) => self.let_bind_nil(sym)?,
                _ => bail!(Error::from_object(Type::Cons, binding)),
            }
            len += 1;
        }
        if parallel {
            let num_unbound_vars = let_bindings.len();
            let stack_size = self.vars.len();
            debug_assert!(stack_size >= num_unbound_vars);
            let binding_start = stack_size - num_unbound_vars;
            self.vars.drain(binding_start..);
            self.vars.append(&mut let_bindings);
        }
        Ok(len)
    }

    fn setq(&mut self, obj: Object<'ob>) -> Result<()> {
        let mut forms = into_iter(obj)?;
        let mut args_processed = 0;
        // (setq)
        ensure!(!forms.is_empty(), Error::ArgCount(2, 0));
        // Iterate over variable/value pairs
        while let Some(var) = forms.next() {
            args_processed += 1;
            // value
            match forms.next() {
                // (setq x y)
                Some(val) => self.compile_form(val?)?,
                // (setq x)
                None => bail!(Error::ArgCount(args_processed + 1, args_processed)),
            }
            args_processed += 1;
            if forms.is_empty() {
                self.duplicate();
            }

            // variable
            let sym = var?.try_into()?;
            match self.vars.iter().rposition(|&x| x == Some(sym)) {
                Some(idx) => self.stack_set(idx)?,
                None => {
                    let idx = self.constants.insert(sym.into())?;
                    self.var_set(idx);
                }
            }
        }
        Ok(())
    }

    fn compile_macro_call(
        &mut self,
        name: Symbol,
        args: Object<'ob>,
        body: Object<'ob>,
    ) -> Result<Object<'ob>> {
        let arena = self.arena;
        let mut arg_list = vec![];
        for arg in into_iter(args)? {
            arg_list.push(arg?);
        }
        match body.val() {
            Value::LispFn(lisp_macro) => eval::call_lisp(lisp_macro, arg_list, self.env, arena),
            Value::SubrFn(lisp_macro) => eval::call_subr(*lisp_macro, arg_list, self.env, arena),
            Value::Cons(macro_form) => {
                if self.env.macro_callstack.iter().any(|&x| x == name) {
                    bail!(CompError::RecursiveMacro);
                }
                self.env.macro_callstack.push(name);

                let lisp_macro = {
                    let func_ident = macro_form.car().as_symbol()?;
                    match func_ident.get_name() {
                        "lambda" => compile_lambda(macro_form.cdr(), self.env, arena)?,
                        bad_function => bail!("Invalid Function : {}", bad_function),
                    }
                };
                self.env.macro_callstack.pop();
                let func = arena.add(lisp_macro);
                let def = cons!(intern("macro"), func; arena);
                crate::data::set_global_function(
                    name,
                    def.try_into().expect("Type should be a valid macro"),
                    self.env,
                );
                if let Object::LispFn(func) = func {
                    eval::call_lisp(!func, arg_list, self.env, arena)
                } else {
                    unreachable!("Compiled function was not lisp fn");
                }
            }
            x => bail!("Invalid macro type: {:?}", x.get_type()),
        }
    }

    fn compile_funcall(&mut self, name: Symbol, args: Object<'ob>) -> Result<()> {
        let callable = crate::data::symbol_function(name, &mut self.env, self.arena);
        if let Object::Cons(cons) = callable {
            match cons.car().val() {
                Value::Symbol(sym) if sym.get_name() == "macro" => {
                    let form = self.compile_macro_call(name, args, cons.cdr())?;
                    self.compile_form(form)?;
                }
                _ => {}
            }
        } else {
            self.const_ref(name.into(), None)?;
            let prev_len = self.vars.len();
            let args = into_iter(args)?;
            let mut num_args = 0;
            for arg in args {
                self.compile_form(arg?)?;
                num_args += 1;
            }
            self.codes.emit_call(num_args as u16);
            self.vars.truncate(prev_len);
        }
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
            panic!("invalid back jump opcode provided: {:?}", jump_code);
        }
    }

    fn compile_if(&mut self, obj: Object<'ob>) -> Result<()> {
        // let list = into_list(obj)?;
        let mut forms = into_iter(obj)?;
        let len = forms.clone().count();
        match len {
            // (if) | (if x)
            0 | 1 => Err(Error::ArgCount(2, len as u16).into()),
            // (if x y)
            2 => {
                self.compile_form(forms.next().unwrap()?)?;
                let target = self.jump(OpCode::JumpNilElsePop);
                self.compile_form(forms.next().unwrap()?)?;
                self.set_jump_target(target);
                Ok(())
            }
            // (if x y z ...)
            _ => {
                self.compile_form(forms.next().unwrap()?)?;
                let else_nil_target = self.jump(OpCode::JumpNil);
                // if branch
                self.compile_form(forms.next().unwrap()?)?;
                let jump_to_end_target = self.jump(OpCode::Jump);
                // else branch
                self.set_jump_target(else_nil_target);
                self.implicit_progn(forms)?;
                self.set_jump_target(jump_to_end_target);
                Ok(())
            }
        }
    }

    fn compile_loop(&mut self, obj: Object<'ob>) -> Result<()> {
        let mut forms = into_iter(obj)?;
        let top = self.codes.len();
        match forms.next() {
            Some(form) => self.compile_form(form?)?,
            None => bail!(Error::ArgCount(1, 0)),
        }
        let loop_exit = self.jump(OpCode::JumpNilElsePop);
        self.implicit_progn(forms)?;
        self.discard();
        self.jump_back(OpCode::Jump, top);
        self.set_jump_target(loop_exit);
        Ok(())
    }

    fn compile_lambda_def(&mut self, obj: Object<'ob>) -> Result<()> {
        let lambda = compile_lambda(obj, self.env, self.arena)?;
        self.add_const_lambda(lambda)
    }

    fn compile_defvar(&mut self, obj: Object<'ob>) -> Result<()> {
        let mut iter = into_iter(obj)?;

        match iter.next() {
            // (defvar x ...)
            Some(x) => {
                let sym = x?.as_symbol()?;
                // TODO: compile this into a lambda like Emacs does
                match iter.next() {
                    // (defvar x y)
                    Some(value) => self.compile_form(value?)?,
                    // (defvar x)
                    None => self.const_ref(Object::Nil, None)?,
                };
                self.duplicate();
                let idx = self.constants.insert(sym.into())?;
                self.var_set(idx);
                Ok(())
            }
            // (defvar)
            None => Err(Error::ArgCount(1, 0).into()),
        }
    }

    fn compile_cond_clause(
        &mut self,
        clause: Object<'ob>,
        jump_targets: &mut Vec<(usize, OpCode)>,
    ) -> Result<()> {
        let mut cond = into_iter(clause)?;
        let len = cond.clone().count();
        match len {
            // (cond ())
            0 => {}
            // (cond (x))
            1 => {
                self.compile_form(cond.next().unwrap()?)?;
                let target = self.jump(OpCode::JumpNotNilElsePop);
                jump_targets.push(target);
            }
            // (cond (x y ...))
            _ => {
                self.compile_form(cond.next().unwrap()?)?;
                let skip_target = self.jump(OpCode::JumpNil);
                self.implicit_progn(cond)?;
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
        clause: Object<'ob>,
        jump_targets: &mut Vec<(usize, OpCode)>,
    ) -> Result<()> {
        let mut cond = into_iter(clause)?;
        let len = cond.clone().count();
        match len {
            // (cond ())
            0 => {
                self.const_ref(Object::Nil, None)?;
            }
            // (cond (x))
            1 => {
                self.compile_form(cond.next().unwrap()?)?;
                let target = self.jump(OpCode::JumpNotNilElsePop);
                self.const_ref(Object::Nil, None)?;
                jump_targets.push(target);
            }
            // (cond (x y ...))
            _ => {
                self.compile_form(cond.next().unwrap()?)?;
                let target = self.jump(OpCode::JumpNilElsePop);
                self.implicit_progn(cond)?;
                jump_targets.push(target);
            }
        }
        Ok(())
    }

    fn compile_cond(&mut self, obj: Object<'ob>) -> Result<()> {
        let mut clauses = into_iter(obj)?;
        // (cond)
        if clauses.is_empty() {
            return self.const_ref(Object::Nil, None);
        }

        let final_return_targets = &mut Vec::new();
        while let Some(clause) = clauses.next() {
            if clauses.is_empty() {
                self.compile_last_cond_clause(clause?, final_return_targets)?;
            } else {
                self.compile_cond_clause(clause?, final_return_targets)?;
            }
        }

        for target in final_return_targets {
            self.codes.set_jump_placeholder(target.0);
        }
        Ok(())
    }

    fn dispatch_special_form(&mut self, cons: &Cons<'ob>) -> Result<()> {
        println!("car = {}", cons.car());
        let name: Symbol = cons.car().try_into()?;
        let forms = cons.cdr();
        match name.get_name() {
            "lambda" => self.compile_lambda_def(forms),
            "while" => self.compile_loop(forms),
            "quote" | "function" => self.quote(forms),
            "progn" => self.progn(forms),
            "setq" => self.setq(forms),
            "defvar" => self.compile_defvar(forms),
            "cond" => self.compile_cond(forms),
            "let" => self.compile_let(forms, true),
            "let*" => self.compile_let(forms, false),
            "if" => self.compile_if(forms),
            _ => self.compile_funcall(name, forms),
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

    fn compile_form(&mut self, obj: Object<'ob>) -> Result<()> {
        // println!("state = {:?}", self.vars);
        // println!("constants = {:?}", self.constants);
        match obj.val() {
            Value::Cons(cons) => self.dispatch_special_form(cons),
            Value::Symbol(sym) => self.variable_reference(sym),
            _ => self.const_ref(obj, None),
        }
    }

    fn compile_func_body(
        obj: ConsIter<'_, 'ob>,
        vars: Vec<Option<Symbol>>,
        env: &'brw mut Environment<'ob>,
        arena: &'ob Arena,
    ) -> Result<Self> {
        let mut exp = Compiler::new(vars, env, arena);
        exp.implicit_progn(obj)?;
        exp.codes.push_op(OpCode::Ret);
        exp.vars.truncate(0);
        Ok(exp)
    }
}

fn parse_fn_binding(bindings: Object) -> Result<(u16, u16, bool, Vec<Symbol>)> {
    let mut args: Vec<Symbol> = vec![];
    let mut required = 0;
    let mut optional = 0;
    let mut rest = false;
    let mut arg_type = &mut required;
    let mut iter = into_iter(bindings)?;
    while let Some(binding) = iter.next() {
        let sym = binding?.as_symbol()?;
        match sym.get_name() {
            "&optional" => arg_type = &mut optional,
            "&rest" => {
                if let Some(last) = iter.next() {
                    rest = true;
                    args.push(last?.as_symbol()?);
                    ensure!(
                        iter.next().is_none(),
                        "Found multiple arguments after &rest"
                    );
                }
            }
            _ => {
                *arg_type += 1;
                args.push(sym);
            }
        }
    }
    Ok((required, optional, rest, args))
}

pub(crate) fn compile_lambda<'ob>(
    obj: Object<'ob>,
    env: &mut Environment<'ob>,
    arena: &'ob Arena,
) -> Result<LispFn<'ob>> {
    let mut iter = into_iter(obj)?;

    let (required, optional, rest, args) = match iter.next() {
        // (lambda ())
        None => return Ok(LispFn::default()),
        // (lambda (x ...) ...)
        Some(bindings) => parse_fn_binding(bindings?)?,
    };
    if iter.is_empty() {
        Ok(LispFn::default())
    } else {
        let func_args = args.into_iter().map(Some).collect();
        let exp: Expression<'ob> = Compiler::compile_func_body(iter, func_args, env, arena)?.into();
        let func = LispFn::new(exp.op_codes, exp.constants, required, optional, rest);
        Ok(func)
    }
}

pub(crate) fn compile<'ob>(
    obj: Object<'ob>,
    env: &mut Environment<'ob>,
    arena: &'ob Arena,
) -> Result<Expression<'ob>> {
    let cons = Cons::new(obj, Object::Nil);
    Compiler::compile_func_body(cons.into_iter(), vec![], env, arena).map(|x| x.into())
}

#[cfg(test)]
mod test {

    use super::*;
    use crate::arena::Arena;
    use crate::reader::Reader;
    use crate::symbol::intern;
    #[allow(clippy::enum_glob_use)]
    use OpCode::*;

    fn check_error<E>(compare: &str, expect: E)
    where
        E: std::error::Error + PartialEq + Send + Sync + 'static,
    {
        let arena = &Arena::new();
        let env = &mut Environment::default();
        let obj = Reader::read(compare, arena).unwrap().0;
        assert_eq!(
            compile(obj, env, arena)
                .err()
                .unwrap()
                .downcast::<E>()
                .unwrap(),
            expect
        );
    }

    macro_rules! check_compiler {
        ($compare:expr, [$($op:expr),+], [$($const:expr),+]) => ({
            let comp_arena = &Arena::new();
            let comp_env = &mut Environment::default();
            println!("Test String: {}", $compare);
            let obj = Reader::read($compare, comp_arena).unwrap().0;
            let expect = Expression {
                op_codes: vec_into![$($op),+].into(),
                constants: vec_into_object![$($const),+; comp_arena],
            };
            assert_eq!(compile(obj, comp_env, comp_arena).unwrap(), expect);
        })
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
        check_error("(let (foo 1))", Error::from_object(Type::Cons, 1.into()));
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
            [Object::Nil, 1, 2]
        );
        check_compiler!(
            "(if t 2)",
            [Constant0, JumpNilElsePop, high1, low1, Constant1, Ret],
            [Object::True, 2]
        );
        check_error("(if 1)", Error::ArgCount(2, 1));
    }

    #[test]
    fn cond_stmt() {
        check_compiler!("(cond)", [Constant0, Ret], [Object::Nil]);
        check_compiler!("(cond ())", [Constant0, Ret], [Object::Nil]);
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
            [Object::True, Object::Nil]
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
            [Object::True, 1]
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
            [Object::Nil, 2]
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
            [Object::Nil, 2, 3]
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
        check_error("(foo . 1)", Error::from_object(Type::List, 1.into()));
    }

    fn check_lambda<'ob>(sexp: &str, func: LispFn<'ob>, comp_arena: &'ob Arena) {
        println!("Test String: {}", sexp);
        let obj = Reader::read(sexp, comp_arena).unwrap().0;
        let env = &mut Environment::default();
        let lambda = match obj {
            Object::Cons(cons) => cons.cdr(),
            x => panic!("expected cons, found {}", x),
        };
        assert_eq!(compile_lambda(lambda, env, comp_arena).unwrap(), func);
    }

    #[test]
    fn lambda() {
        let arena = &Arena::new();
        let env = &mut Environment::default();
        check_lambda("(lambda)", LispFn::default(), arena);
        check_lambda("(lambda ())", LispFn::default(), arena);
        check_lambda("(lambda () nil)", LispFn::default(), arena);

        check_lambda(
            "(lambda () 1)",
            LispFn::new(vec_into![Constant0, Ret].into(), vec_into![1], 0, 0, false),
            arena,
        );

        check_lambda(
            "(lambda (x) x)",
            LispFn::new(vec_into![StackRef0, Ret].into(), vec![], 1, 0, false),
            arena,
        );

        check_lambda(
            "(lambda (x &optional) x)",
            LispFn::new(vec_into![StackRef0, Ret].into(), vec![], 1, 0, false),
            arena,
        );
        check_lambda(
            "(lambda (x &optional y) x)",
            LispFn::new(vec_into![StackRef1, Ret].into(), vec![], 1, 1, false),
            arena,
        );
        check_lambda(
            "(lambda (x &optional y z) y)",
            LispFn::new(vec_into![StackRef1, Ret].into(), vec![], 1, 2, false),
            arena,
        );
        check_lambda(
            "(lambda (x &optional y &optional z) z)",
            LispFn::new(vec_into![StackRef0, Ret].into(), vec![], 1, 2, false),
            arena,
        );
        check_lambda(
            "(lambda (x &rest) x)",
            LispFn::new(vec_into![StackRef0, Ret].into(), vec![], 1, 0, false),
            arena,
        );
        check_lambda(
            "(lambda (x &rest y) y)",
            LispFn::new(vec_into![StackRef0, Ret].into(), vec![], 1, 0, true),
            arena,
        );

        let obj = Reader::read("(lambda (x &rest y z) y)", arena).unwrap().0;
        assert!(compile(obj, env, arena)
            .err()
            .unwrap()
            .downcast::<&str>()
            .is_ok());

        check_lambda(
            "(lambda (x y) (+ x y))",
            LispFn::new(
                vec_into![Constant0, StackRef2, StackRef2, Call2, Ret].into(),
                vec_into![intern("+")],
                2,
                0,
                false,
            ),
            arena,
        );

        let func = LispFn::new(vec_into![StackRef0, Ret].into(), vec![], 1, 0, false);
        check_compiler!(
            "(let ((x 1)(y 2)) (lambda (x) x))",
            [Constant0, Constant1, Constant2, DiscardNKeepTOS, 2, Ret],
            [1, 2, func]
        );

        check_error(
            "(lambda (x 1) x)",
            Error::from_object(Type::Symbol, 1.into()),
        );
    }

    #[test]
    fn errors() {
        check_error(
            "(\"foo\")",
            Error::Type(Type::Symbol, Type::String, "\"foo\"".to_owned()),
        );
        check_error("(quote)", Error::ArgCount(1, 0));
        check_error("(quote 1 2)", Error::ArgCount(1, 2));
    }

    #[test]
    fn let_errors() {
        check_error("(let (1))", Error::from_object(Type::Cons, 1.into()));
        check_error("(let ((foo 1 2)))", CompError::LetValueCount(2));
        check_error(
            "(let ((foo . 1)))",
            Error::from_object(Type::List, 1.into()),
        );
        check_error("(let (()))", Error::from_object(Type::Cons, Object::Nil));
        check_error("(let)", Error::ArgCount(1, 0));
    }
}
