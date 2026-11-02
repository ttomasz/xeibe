//! A CRS as the Python side hands it over: its PROJJSON dict, read into a
//! `serde_json::Value` as it is, without the text `json.dumps` would make
//! and Rust would parse again, or text (PROJJSON, WKT or a PROJ string).

use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use pyo3::pybacked::PyBackedStr;
use pyo3::types::{PyBool, PyDict, PyFloat, PyInt, PyList, PyString, PyTuple};
use serde_json::{Map, Number, Value};

pub(crate) enum Source {
    Value(Value),
    Text(PyBackedStr),
}

impl FromPyObject<'_, '_> for Source {
    type Error = PyErr;

    fn extract(obj: Borrowed<'_, '_, PyAny>) -> PyResult<Self> {
        if obj.is_instance_of::<PyDict>() {
            Ok(Source::Value(value(&obj)?))
        } else {
            Ok(Source::Text(obj.extract()?))
        }
    }
}

/// `obj` as JSON, as `json.dumps` writes it and `serde_json` reads that
/// back: tuples as arrays, integers beyond 64 bits as floats, the
/// `TypeError` `json.dumps` raises for other objects and keys that aren't
/// strings, and the `CRSError` reading its NaN and infinity was.
fn value(obj: &Bound<'_, PyAny>) -> PyResult<Value> {
    if let Ok(s) = obj.cast::<PyString>() {
        return Ok(Value::String(s.to_str()?.to_owned()));
    }
    if let Ok(d) = obj.cast::<PyDict>() {
        let mut map = Map::new();
        for (k, v) in d.iter() {
            let key = k
                .cast::<PyString>()
                .map_err(|_| PyTypeError::new_err("PROJJSON keys must be strings"))?;
            map.insert(key.to_str()?.to_owned(), value(&v)?);
        }
        return Ok(Value::Object(map));
    }
    if let Ok(l) = obj.cast::<PyList>() {
        return l.iter().map(|v| value(&v)).collect();
    }
    if let Ok(t) = obj.cast::<PyTuple>() {
        return t.iter().map(|v| value(&v)).collect();
    }
    if obj.is_none() {
        return Ok(Value::Null);
    }
    // Before integers: bool is a subclass of int.
    if let Ok(b) = obj.cast::<PyBool>() {
        return Ok(Value::Bool(b.is_true()));
    }
    if obj.is_instance_of::<PyInt>() {
        if let Ok(i) = obj.extract::<i64>() {
            return Ok(i.into());
        }
        if let Ok(u) = obj.extract::<u64>() {
            return Ok(u.into());
        }
        return number(obj.py(), obj.extract::<f64>()?);
    }
    if let Ok(f) = obj.cast::<PyFloat>() {
        return number(obj.py(), f.value());
    }
    Err(PyTypeError::new_err(format!(
        "Object of type {} is not JSON serializable",
        obj.get_type().name()?
    )))
}

fn number(py: Python<'_>, f: f64) -> PyResult<Value> {
    Number::from_f64(f)
        .map(Value::Number)
        .ok_or_else(|| crate::exception(py, "CRSError", format!("PROJJSON: {f} is not a number")))
}
