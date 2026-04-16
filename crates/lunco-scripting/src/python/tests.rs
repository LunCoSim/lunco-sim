#[cfg(test)]
mod tests {
    use crate::python;
    use crate::python::reflect::EntityProxy;
    use pyo3::prelude::*;
    use pyo3::IntoPyObjectExt;
    use std::ffi::CString;

    #[test]
    fn test_python_entity_proxy() {
        python::initialize_python();
        Python::with_gil(|py| {
            let sys = py.import("sys").unwrap();
            println!("Python version: {}", sys.getattr("version").unwrap());

            // Test our EntityProxy
            let locals = pyo3::types::PyDict::new(py);
            let proxy = EntityProxy::new(42);
            locals.set_item("entity", proxy.into_py_any(py).unwrap()).unwrap();

            let code = CString::new("entity.Transform").unwrap();
            let result: String = py.eval(&code, None, Some(&locals)).unwrap().extract().unwrap();
            println!("Result: {}", result);
            assert!(result.contains("Component 'Transform' on"));
            assert!(result.contains("42"));
        });
    }
}
