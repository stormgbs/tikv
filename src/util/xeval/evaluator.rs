// Copyright 2016 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// See the License for the specific language governing permissions and
// limitations under the License.


use util::codec::{number, Datum, datum};
use util::TryInsertWith;
use super::{Result, Error};

use std::collections::HashMap;
use std::cmp::Ordering;
use std::ascii::AsciiExt;
use tipb::expression::{Expr, ExprType};

/// `Evaluator` evaluates `tipb::Expr`.
#[derive(Default)]
pub struct Evaluator {
    // column_id -> column_value
    pub row: HashMap<i64, Datum>,
    // expr pointer -> value list
    cached_value_list: HashMap<isize, Vec<Datum>>,
}

impl Evaluator {
    /// Eval evaluates expr to a Datum.
    pub fn eval(&mut self, expr: &Expr) -> Result<Datum> {
        match expr.get_tp() {
            ExprType::Int64 => self.eval_int(expr),
            ExprType::Uint64 => self.eval_uint(expr),
            // maybe we should use take here?
            ExprType::String | ExprType::Bytes => Ok(Datum::Bytes(expr.get_val().to_vec())),
            ExprType::ColumnRef => self.eval_column_ref(expr),
            ExprType::LT => self.eval_lt(expr),
            ExprType::LE => self.eval_le(expr),
            ExprType::EQ => self.eval_eq(expr),
            ExprType::NE => self.eval_ne(expr),
            ExprType::GE => self.eval_ge(expr),
            ExprType::GT => self.eval_gt(expr),
            ExprType::NullEQ => self.eval_null_eq(expr),
            ExprType::And => self.eval_and(expr),
            ExprType::Or => self.eval_or(expr),
            ExprType::Not => self.eval_not(expr),
            ExprType::Like => self.eval_like(expr),
            ExprType::Float32 |
            ExprType::Float64 => unimplemented!(),
            ExprType::In => self.eval_in(expr),
            _ => Ok(Datum::Null),
        }
    }

    fn eval_int(&self, expr: &Expr) -> Result<Datum> {
        let i = try!(number::decode_i64(expr.get_val()));
        Ok(Datum::I64(i))
    }

    fn eval_uint(&self, expr: &Expr) -> Result<Datum> {
        let u = try!(number::decode_u64(expr.get_val()));
        Ok(Datum::U64(u))
    }

    fn eval_column_ref(&self, expr: &Expr) -> Result<Datum> {
        let i = try!(number::decode_i64(expr.get_val()));
        self.row.get(&i).cloned().ok_or_else(|| Error::Eval(format!("column {} not found", i)))
    }

    fn eval_lt(&mut self, expr: &Expr) -> Result<Datum> {
        let cmp = try!(self.cmp_children(expr));
        Ok(cmp.map(|c| c < Ordering::Equal).into())
    }

    fn eval_le(&mut self, expr: &Expr) -> Result<Datum> {
        let cmp = try!(self.cmp_children(expr));
        Ok(cmp.map(|c| c <= Ordering::Equal).into())
    }

    fn eval_eq(&mut self, expr: &Expr) -> Result<Datum> {
        let cmp = try!(self.cmp_children(expr));
        Ok(cmp.map(|c| c == Ordering::Equal).into())
    }

    fn eval_ne(&mut self, expr: &Expr) -> Result<Datum> {
        let cmp = try!(self.cmp_children(expr));
        Ok(cmp.map(|c| c != Ordering::Equal).into())
    }

    fn eval_ge(&mut self, expr: &Expr) -> Result<Datum> {
        let cmp = try!(self.cmp_children(expr));
        Ok(cmp.map(|c| c >= Ordering::Equal).into())
    }

    fn eval_gt(&mut self, expr: &Expr) -> Result<Datum> {
        let cmp = try!(self.cmp_children(expr));
        Ok(cmp.map(|c| c > Ordering::Equal).into())
    }

    fn eval_null_eq(&mut self, expr: &Expr) -> Result<Datum> {
        let (left, right) = try!(self.eval_two_children(expr));
        let cmp = try!(left.cmp(&right));
        Ok((cmp == Ordering::Equal).into())
    }

    fn cmp_children(&mut self, expr: &Expr) -> Result<Option<Ordering>> {
        let (left, right) = try!(self.eval_two_children(expr));
        if left == Datum::Null || right == Datum::Null {
            return Ok(None);
        }
        left.cmp(&right).map(Some).map_err(From::from)
    }

    fn eval_two_children(&mut self, expr: &Expr) -> Result<(Datum, Datum)> {
        let l = expr.get_children().len();
        if l != 2 {
            return Err(Error::Expr(format!("need 2 operands but got {}", l)));
        }
        let children = expr.get_children();
        let left = try!(self.eval(&children[0]));
        let right = try!(self.eval(&children[1]));
        Ok((left, right))
    }

    fn eval_and(&mut self, expr: &Expr) -> Result<Datum> {
        self.eval_two_children_as_bool(expr)
            .map(|p| {
                match p {
                    (Some(true), Some(true)) => true.into(),
                    (Some(false), _) | (_, Some(false)) => false.into(),
                    _ => Datum::Null,
                }
            })
    }

    fn eval_or(&mut self, expr: &Expr) -> Result<Datum> {
        self.eval_two_children_as_bool(expr).map(|p| {
            match p {
                (Some(true), _) | (_, Some(true)) => true.into(),
                (Some(false), Some(false)) => false.into(),
                _ => Datum::Null,
            }
        })
    }

    fn eval_not(&mut self, expr: &Expr) -> Result<Datum> {
        let children_cnt = expr.get_children().len();
        if children_cnt != 1 {
            return Err(Error::Expr(format!("expect 1 operand, got {}", children_cnt)));
        }
        let d = try!(self.eval(&expr.get_children()[0]));
        if d == Datum::Null {
            return Ok(Datum::Null);
        }
        let b = try!(d.into_bool());
        Ok((!b).into())
    }

    fn eval_like(&mut self, expr: &Expr) -> Result<Datum> {
        let (target, pattern) = try!(self.eval_two_children(expr));
        if Datum::Null == target || Datum::Null == pattern {
            return Ok(Datum::Null);
        }
        let mut target_str = try!(target.into_string());
        let mut pattern_str = try!(pattern.into_string());
        if pattern_str.chars().any(|x| x.is_ascii() && x.is_alphabetic()) {
            target_str = target_str.to_ascii_lowercase();
            pattern_str = pattern_str.to_ascii_lowercase();
        }
        // for now, tidb ensures that pattern being pushed down must match ^%?[^\\_%]*%?$.
        let len = pattern_str.len();
        if pattern_str.starts_with('%') {
            if pattern_str[1..].ends_with('%') {
                Ok(target_str.contains(&pattern_str[1..len - 1]).into())
            } else {
                Ok(target_str.ends_with(&pattern_str[1..]).into())
            }
        } else if pattern_str.ends_with('%') {
            Ok(target_str.starts_with(&pattern_str[..len - 1]).into())
        } else {
            Ok(target_str.eq(&pattern_str).into())
        }
    }

    fn eval_two_children_as_bool(&mut self, expr: &Expr) -> Result<(Option<bool>, Option<bool>)> {
        let (left, right) = try!(self.eval_two_children(expr));
        let left_bool = try!(eval_into_bool(left));
        let right_bool = try!(eval_into_bool(right));
        Ok((left_bool, right_bool))
    }

    fn eval_in(&mut self, expr: &Expr) -> Result<Datum> {
        if expr.get_children().len() != 2 {
            return Err(Error::Expr(format!("IN need 2 operand, got {}",
                                           expr.get_children().len())));
        }
        let children = expr.get_children();
        let target = try!(self.eval(&children[0]));
        if let Datum::Null = target {
            return Ok(target);
        }
        let value_list_expr = &children[1];
        if value_list_expr.get_tp() != ExprType::ValueList {
            return Err(Error::Expr("the second children should be value list type".to_owned()));
        }
        let decoded = try!(self.decode_value_list(value_list_expr));
        if try!(check_in(target, decoded)) {
            return Ok(true.into());
        }
        if decoded.first().map_or(false, |d| *d == Datum::Null) {
            return Ok(Datum::Null);
        }
        Ok(false.into())
    }

    fn decode_value_list(&mut self, value_list_expr: &Expr) -> Result<&Vec<Datum>> {
        let p = value_list_expr as *const Expr as isize;
        let decoded = try!(self.cached_value_list
                               .entry(p)
                               .or_try_insert_with(|| datum::decode(value_list_expr.get_val())));
        Ok(decoded)
    }
}

/// eval datum into bool, if expr is Null, then None is return.
fn eval_into_bool(datum: Datum) -> Result<Option<bool>> {
    if datum == Datum::Null {
        Ok(None)
    } else {
        let b = try!(datum.into_bool());
        Ok(Some(b))
    }
}

/// Check if `target` is in `value_list`.
fn check_in(target: Datum, value_list: &[Datum]) -> Result<bool> {
    let mut err = None;
    let pos = value_list.binary_search_by(|d| {
        match d.cmp(&target) {
            Ok(ord) => ord,
            Err(e) => {
                err = Some(e);
                Ordering::Less
            }
        }
    });
    if let Some(e) = err {
        return Err(e.into());
    }
    Ok(pos.is_ok())
}

#[cfg(test)]
mod test {
    use super::*;
    use util::codec::{Datum, number, datum};

    use tipb::expression::{Expr, ExprType};
    use protobuf::RepeatedField;

    fn datum_expr(datum: Datum) -> Expr {
        let mut expr = Expr::new();
        match datum {
            Datum::I64(i) => {
                expr.set_tp(ExprType::Int64);
                let mut buf = vec![0; 8];
                number::encode_i64(&mut buf, i).unwrap();
                expr.set_val(buf);
            }
            Datum::U64(u) => {
                expr.set_tp(ExprType::Uint64);
                let mut buf = vec![0; 8];
                number::encode_u64(&mut buf, u).unwrap();
                expr.set_val(buf);
            }
            Datum::Bytes(bs) => {
                expr.set_tp(ExprType::Bytes);
                expr.set_val(bs);
            }
            Datum::F32(_) => unimplemented!(),
            Datum::F64(_) => unimplemented!(),
            _ => expr.set_tp(ExprType::Null),
        };
        expr
    }

    fn col_expr(col_id: i64) -> Expr {
        let mut expr = Expr::new();
        expr.set_tp(ExprType::ColumnRef);
        let mut buf = vec![0; 8];
        number::encode_i64(&mut buf, col_id).unwrap();
        expr.set_val(buf);
        expr
    }

    fn bin_expr(left: Datum, right: Datum, tp: ExprType) -> Expr {
        bin_expr_r(datum_expr(left), datum_expr(right), tp)
    }

    fn bin_expr_r(left: Expr, right: Expr, tp: ExprType) -> Expr {
        let mut expr = Expr::new();
        expr.set_tp(tp);
        expr.set_children(RepeatedField::from_vec(vec![left, right]));
        expr
    }

    fn not_expr(value: Datum) -> Expr {
        let mut expr = Expr::new();
        expr.set_tp(ExprType::Not);
        expr.mut_children().push(datum_expr(value));
        expr
    }

    fn like_expr(target: &'static str, pattern: &'static str) -> Expr {
        let target_expr = datum_expr(Datum::Bytes(target.as_bytes().to_vec()));
        let pattern_expr = datum_expr(Datum::Bytes(pattern.as_bytes().to_vec()));
        let mut expr = Expr::new();
        expr.set_tp(ExprType::Like);
        expr.mut_children().push(target_expr);
        expr.mut_children().push(pattern_expr);
        expr
    }

    // TODO: add more tests.
    #[test]
    fn test_eval() {
        let tests = vec![
			(datum_expr(Datum::I64(1)), Datum::I64(1)),
			(datum_expr(Datum::U64(1)), Datum::U64(1)),
			(datum_expr(b"abc".as_ref().into()), b"abc".as_ref().into()),
			(datum_expr(Datum::Null), Datum::Null),
			(col_expr(1), Datum::I64(100)),
			(bin_expr(Datum::I64(100), Datum::I64(1), ExprType::LT), Datum::I64(0)),
			(bin_expr(Datum::I64(1), Datum::I64(100), ExprType::LT), Datum::I64(1)),
			(bin_expr(Datum::I64(100), Datum::Null, ExprType::LT), Datum::Null),
			(bin_expr(Datum::I64(100), Datum::I64(1), ExprType::LE), Datum::I64(0)),
			(bin_expr(Datum::I64(1), Datum::I64(1), ExprType::LE), Datum::I64(1)),
			(bin_expr(Datum::I64(100), Datum::Null, ExprType::LE), Datum::Null),
			(bin_expr(Datum::I64(100), Datum::I64(1), ExprType::EQ), Datum::I64(0)),
			(bin_expr(Datum::I64(100), Datum::I64(100), ExprType::EQ), Datum::I64(1)),
			(bin_expr(Datum::I64(100), Datum::Null, ExprType::EQ), Datum::Null),
			(bin_expr(Datum::I64(100), Datum::I64(100), ExprType::NE), Datum::I64(0)),
			(bin_expr(Datum::I64(100), Datum::I64(1), ExprType::NE), Datum::I64(1)),
			(bin_expr(Datum::I64(100), Datum::Null, ExprType::NE), Datum::Null),
			(bin_expr(Datum::I64(1), Datum::I64(100), ExprType::GE), Datum::I64(0)),
			(bin_expr(Datum::I64(100), Datum::I64(100), ExprType::GE), Datum::I64(1)),
			(bin_expr(Datum::I64(100), Datum::Null, ExprType::GE), Datum::Null),
			(bin_expr(Datum::I64(100), Datum::I64(100), ExprType::GT), Datum::I64(0)),
			(bin_expr(Datum::I64(100), Datum::I64(1), ExprType::GT), Datum::I64(1)),
			(bin_expr(Datum::I64(100), Datum::Null, ExprType::GT), Datum::Null),
			(bin_expr(Datum::I64(1), Datum::Null, ExprType::NullEQ), Datum::I64(0)),
			(bin_expr(Datum::Null, Datum::Null, ExprType::NullEQ), Datum::I64(1)),
			// logic operation
			(bin_expr(Datum::I64(0), Datum::I64(1), ExprType::And), Datum::I64(0)),
			(bin_expr(Datum::I64(1), Datum::I64(1), ExprType::And), Datum::I64(1)),
			(bin_expr(Datum::I64(1), Datum::Null, ExprType::And), Datum::Null),
			(bin_expr(Datum::Null, Datum::I64(0), ExprType::And), Datum::I64(0)),
			(bin_expr(Datum::Null, Datum::Null, ExprType::And), Datum::Null),
			(bin_expr(Datum::I64(0), Datum::I64(0), ExprType::Or), Datum::I64(0)),
			(bin_expr(Datum::I64(0), Datum::I64(1), ExprType::Or), Datum::I64(1)),
			(bin_expr(Datum::I64(1), Datum::Null, ExprType::Or), Datum::I64(1)),
			(bin_expr(Datum::Null, Datum::Null, ExprType::Or), Datum::Null),
			(bin_expr(Datum::Null, Datum::I64(0), ExprType::Or), Datum::Null),
			(bin_expr_r(bin_expr(Datum::I64(1), Datum::I64(1), ExprType::EQ),
			 bin_expr(Datum::I64(1), Datum::I64(1), ExprType::EQ), ExprType::And), Datum::I64(1)),
			(not_expr(Datum::I64(1)), Datum::I64(0)),
			(not_expr(Datum::I64(0)), Datum::I64(1)),
			(not_expr(Datum::Null), Datum::Null),
            // like operation
            (like_expr("a", ""), Datum::I64(0)),
            (like_expr("a", "a"), Datum::I64(1)),
            (like_expr("a", "b"), Datum::I64(0)),
            (like_expr("aAb", "AaB"), Datum::I64(1)),
            (like_expr("a", "%"), Datum::I64(1)),
            (like_expr("aAD", "%d"), Datum::I64(1)),
            (like_expr("aAeD", "%e"), Datum::I64(0)),
            (like_expr("aAb", "Aa%"), Datum::I64(1)),
            (like_expr("abAb", "Aa%"), Datum::I64(0)),
            (like_expr("aAcb", "%C%"), Datum::I64(1)),
            (like_expr("aAb", "%C%"), Datum::I64(0)),
        ];

        let mut xevaluator = Evaluator::default();
        xevaluator.row.insert(1, Datum::I64(100));
        for (expr, result) in tests {
            let res = xevaluator.eval(&expr);
            if res.is_err() {
                panic!("failed to eval {:?}: {:?}", expr, res);
            }
            let res = res.unwrap();
            if res != result {
                panic!("failed to eval {:?} expect {:?}, got {:?}",
                       expr,
                       result,
                       res);
            }
        }
    }

    fn in_expr(target: Datum, mut list: Vec<Datum>) -> Expr {
        let target_expr = datum_expr(target);
        list.sort_by(|l, r| l.cmp(r).unwrap());
        let val = datum::encode_value(&list).unwrap();
        let mut list_expr = Expr::new();
        list_expr.set_tp(ExprType::ValueList);
        list_expr.set_val(val);
        let mut expr = Expr::new();
        expr.set_tp(ExprType::In);
        expr.mut_children().push(target_expr);
        expr.mut_children().push(list_expr);
        expr
    }

    #[test]
    fn test_where_in() {
        let cases = vec![
            (in_expr(Datum::I64(1), vec![Datum::I64(1), Datum::I64(2)]), Datum::I64(1)),
            (in_expr(Datum::I64(1), vec![Datum::I64(2), Datum::Null]), Datum::Null),
            (in_expr(Datum::Null, vec![Datum::I64(1), Datum::Null]), Datum::Null),
            (in_expr(Datum::I64(2), vec![Datum::I64(1), Datum::Null]), Datum::Null),
            (in_expr(Datum::I64(2), vec![]), Datum::I64(0)),
            (in_expr(b"abc".as_ref().into(), vec![b"abc".as_ref().into(),
             b"ab".as_ref().into()]), Datum::I64(1)),
            (in_expr(b"abc".as_ref().into(), vec![b"aba".as_ref().into(),
             b"bab".as_ref().into()]), Datum::I64(0)),
        ];

        let mut eval = Evaluator::default();
        for (expr, expect_res) in cases {
            let res = eval.eval(&expr);
            if res.is_err() {
                panic!("failed to execute {:?}: {:?}", expr, res);
            }
            let res = res.unwrap();
            if res != expect_res {
                panic!("wrong result {:?}, expect {:?} while executing {:?}",
                       res,
                       expect_res,
                       expr);
            }
        }
    }
}
