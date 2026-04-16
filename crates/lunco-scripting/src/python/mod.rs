pub mod reflect;
#[cfg(test)]
mod tests;

use pyo3::prelude::*;

#[pymodule]
pub fn lunco(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<reflect::EntityProxy>()?;
    Ok(())
}

pub fn initialize_python() {
    pyo3::prepare_freethreaded_python();
}
