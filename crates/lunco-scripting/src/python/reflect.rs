use pyo3::prelude::*;
use pyo3::IntoPyObjectExt;
use bevy::prelude::*;

#[pyclass]
#[derive(Clone)]
pub struct EntityProxy {
    pub entity: Entity,
}

#[pymethods]
impl EntityProxy {
    #[new]
    pub fn new(index: u32) -> Self {
        Self {
            entity: Entity::from_raw_u32(index).unwrap(),
        }
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
