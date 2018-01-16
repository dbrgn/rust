// Copyright 2012-2016 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use rustc::middle::const_val::ConstVal::*;
use rustc::middle::const_val::ErrKind::*;
use rustc::middle::const_val::{ConstVal, ErrKind};

use rustc::hir::def_id::DefId;
use rustc::ty::{self, Ty, TyCtxt};
use rustc::ty::subst::Substs;

use syntax::ast;

use std::cmp::Ordering;

use rustc_const_math::*;

/// * `DefId` is the id of the constant.
/// * `Substs` is the monomorphized substitutions for the expression.
pub fn lookup_const_by_id<'a, 'tcx>(tcx: TyCtxt<'a, 'tcx, 'tcx>,
                                    key: ty::ParamEnvAnd<'tcx, (DefId, &'tcx Substs<'tcx>)>)
                                    -> Option<(DefId, &'tcx Substs<'tcx>)> {
    ty::Instance::resolve(
        tcx,
        key.param_env,
        key.value.0,
        key.value.1,
    ).map(|instance| (instance.def_id(), instance.substs))
}

pub fn lit_to_const<'a, 'tcx>(lit: &'tcx ast::LitKind,
                          tcx: TyCtxt<'a, 'tcx, 'tcx>,
                          ty: Ty<'tcx>,
                          neg: bool)
                          -> Result<ConstVal<'tcx>, ErrKind<'tcx>> {
    use syntax::ast::*;

    use rustc::mir::interpret::*;
    let lit = match *lit {
        LitKind::Str(ref s, _) => {
            let s = s.as_str();
            let id = tcx.allocate_cached(s.as_bytes());
            let ptr = MemoryPointer::new(id, 0);
            Value::ByValPair(
                PrimVal::Ptr(ptr),
                PrimVal::from_u128(s.len() as u128),
            )
        },
        LitKind::ByteStr(ref data) => {
            let id = tcx.allocate_cached(data);
            let ptr = MemoryPointer::new(id, 0);
            Value::ByVal(PrimVal::Ptr(ptr))
        },
        LitKind::Byte(n) => Value::ByVal(PrimVal::Bytes(n as u128)),
        LitKind::Int(n, _) => {
            enum Int {
                Signed(IntTy),
                Unsigned(UintTy),
            }
            let ty = match ty.sty {
                ty::TyInt(IntTy::Isize) => Int::Signed(tcx.sess.target.isize_ty),
                ty::TyInt(other) => Int::Signed(other),
                ty::TyUint(UintTy::Usize) => Int::Unsigned(tcx.sess.target.usize_ty),
                ty::TyUint(other) => Int::Unsigned(other),
                _ => bug!(),
            };
            let n = match ty {
                // FIXME(oli-obk): are these casts correct?
                Int::Signed(IntTy::I8) if neg =>
                    (n as i128 as i8).overflowing_neg().0 as i128 as u128,
                Int::Signed(IntTy::I16) if neg =>
                    (n as i128 as i16).overflowing_neg().0 as i128 as u128,
                Int::Signed(IntTy::I32) if neg =>
                    (n as i128 as i32).overflowing_neg().0 as i128 as u128,
                Int::Signed(IntTy::I64) if neg =>
                    (n as i128 as i64).overflowing_neg().0 as i128 as u128,
                Int::Signed(IntTy::I128) if neg =>
                    (n as i128).overflowing_neg().0 as u128,
                Int::Signed(IntTy::I8) => n as i128 as i8 as i128 as u128,
                Int::Signed(IntTy::I16) => n as i128 as i16 as i128 as u128,
                Int::Signed(IntTy::I32) => n as i128 as i32 as i128 as u128,
                Int::Signed(IntTy::I64) => n as i128 as i64 as i128 as u128,
                Int::Signed(IntTy::I128) => n,
                Int::Unsigned(UintTy::U8) => n as u8 as u128,
                Int::Unsigned(UintTy::U16) => n as u16 as u128,
                Int::Unsigned(UintTy::U32) => n as u32 as u128,
                Int::Unsigned(UintTy::U64) => n as u64 as u128,
                Int::Unsigned(UintTy::U128) => n,
                _ => bug!(),
            };
            Value::ByVal(PrimVal::Bytes(n))
        },
        LitKind::Float(n, fty) => {
            let n = n.as_str();
            let mut f = parse_float(&n, fty)?;
            if neg {
                f = -f;
            }
            let bits = f.bits;
            Value::ByVal(PrimVal::Bytes(bits))
        }
        LitKind::FloatUnsuffixed(n) => {
            let fty = match ty.sty {
                ty::TyFloat(fty) => fty,
                _ => bug!()
            };
            let n = n.as_str();
            let mut f = parse_float(&n, fty)?;
            if neg {
                f = -f;
            }
            let bits = f.bits;
            Value::ByVal(PrimVal::Bytes(bits))
        }
        LitKind::Bool(b) => Value::ByVal(PrimVal::Bytes(b as u128)),
        LitKind::Char(c) => Value::ByVal(PrimVal::Bytes(c as u128)),
    };
    Ok(ConstVal::Value(lit))
}

fn parse_float<'tcx>(num: &str, fty: ast::FloatTy)
                     -> Result<ConstFloat, ErrKind<'tcx>> {
    ConstFloat::from_str(num, fty).map_err(|_| {
        // FIXME(#31407) this is only necessary because float parsing is buggy
        UnimplementedConstVal("could not evaluate float literal (see issue #31407)")
    })
}

pub fn compare_const_vals(a: &ConstVal, b: &ConstVal, ty: Ty) -> Option<Ordering> {
    trace!("compare_const_vals: {:?}, {:?}", a, b);
    use rustc::mir::interpret::{Value, PrimVal};
    match (a, b) {
        (&Value(Value::ByVal(PrimVal::Bytes(a))),
         &Value(Value::ByVal(PrimVal::Bytes(b)))) => {
            match ty.sty {
                ty::TyFloat(ty) => {
                    let l = ConstFloat {
                        bits: a,
                        ty,
                    };
                    let r = ConstFloat {
                        bits: b,
                        ty,
                    };
                    // FIXME(oli-obk): report cmp errors?
                    l.try_cmp(r).ok()
                },
                ty::TyInt(_) => Some((a as i128).cmp(&(b as i128))),
                _ => Some(a.cmp(&b)),
            }
        },
        _ if a == b => Some(Ordering::Equal),
        _ => None,
    }
}
