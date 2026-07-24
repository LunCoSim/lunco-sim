#[cfg(feature = "python")]
use bevy::prelude::*;
#[cfg(feature = "python")]
use pyo3::prelude::*;
#[cfg(feature = "python")]
use pyo3::IntoPyObjectExt;

#[cfg(feature = "python")]
#[pyclass]
#[derive(Clone)]
pub struct EntityProxy {
    pub entity: Entity,
}

#[cfg(feature = "python")]
#[pymethods]
impl EntityProxy {
    #[new]
    pub fn new(index: u32) -> PyResult<Self> {
        // Surface an invalid index as a Python ValueError rather than
        // `unwrap()`-panicking the host: `from_raw_u32` rejects the
        // reserved placeholder slot, and `index` comes straight from
        // user script input.
        let entity = Entity::from_raw_u32(index).ok_or_else(|| {
            pyo3::exceptions::PyValueError::new_err(format!("invalid entity index {index}"))
        })?;
        Ok(Self { entity })
    }

    fn __repr__(&self) -> String {
        format!("EntityProxy({})", self.entity)
    }

    fn __getattr__(&self, name: &str) -> PyResult<PyObject> {
        // This is a simplified version. In a real implementation, we would:
        // 1. Get the World from a thread-local or global resource.
        // 2. Get the TypeRegistry.
        // 3. Find the component 'name' on the entity.
        // 4. Return a reflected Python object.
        Python::with_gil(|py| {
            Ok(format!("Component '{}' on {:?}", name, self.entity).into_py_any(py)?)
        })
    }
}
