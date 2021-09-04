use crate::object::{Expression, LispFn, Object};
use anyhow::{ensure, Result};
use fn_macros::defun;

#[defun]
fn make_closure<'ob>(prototype: &LispFn<'ob>, closure_vars: &[Object<'ob>]) -> Result<LispFn<'ob>> {
    let const_len = prototype.body.constants.len();
    let vars = closure_vars.len();
    ensure!(
        vars <= 5 && vars <= const_len,
        "Closure vars do not fit in const vec"
    );
    let mut constants = prototype.body.constants.clone();
    let zipped = constants.iter_mut().zip(closure_vars.iter());
    for (cnst, var) in zipped {
        *cnst = *var;
    }

    Ok(LispFn {
        body: Expression {
            op_codes: prototype.body.op_codes.clone(),
            constants,
        },
        args: prototype.args,
    })
}

defsubr!(make_closure);
