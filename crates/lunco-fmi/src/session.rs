//! FMI calls and archive admission belong to the worker, never a live ECS borrow.

use fmi_rs::model_description::fmi3::{Causality, ModelDescription, VariableType};
use fmi_sys::fmi3 as binding;
use std::{
    cell::Cell,
    collections::BTreeMap,
    ffi::{CStr, CString},
    io::Cursor,
    ptr,
    rc::Rc,
};

pub(crate) type Values = Vec<(String, f64)>;
pub(crate) type FatalRegistry = BTreeMap<(String, String), Rc<Cell<bool>>>;

#[derive(Clone, Debug)]
pub(crate) struct Description {
    pub name: String,
    pub inputs: Values,
    pub outputs: Values,
    pub dependencies: BTreeMap<String, Vec<String>>,
    pub rollback: bool,
    pub contracts: lunco_port_core::ScalarPortContracts,
}

/// One thread owns extracted resources, native instances and saved states.
/// Generated FMI bindings keep logging, Fatal status and transaction lifetime
/// under the same native owner. No native pointer crosses the ECS boundary.
pub(crate) struct Session {
    binding: binding::Fmi3Binding,
    instance: binding::fmi3Instance,
    _resources: tempfile::TempDir,
    inputs: BTreeMap<String, u32>,
    outputs: BTreeMap<String, u32>,
    event_mode: bool,
    variable_step: bool,
    next_event_time: Option<f64>,
    input_values: BTreeMap<String, f64>,
    time: f64,
    communication_step: f64,
    time_unit: lunco_engineering_values::Unit,
    max_event_iterations: usize,
    fatal: Rc<Cell<bool>>,
    contracts: lunco_port_core::ScalarPortContracts,
}

fn status(value: binding::fmi3Status) -> Result<(), String> {
    match value {
        binding::fmi3Status_fmi3OK => Ok(()),
        binding::fmi3Status_fmi3Warning => {
            bevy::log::warn!("[fmi] FMU returned Warning");
            Ok(())
        }
        binding::fmi3Status_fmi3Discard => Err("FMU discarded its transaction".into()),
        binding::fmi3Status_fmi3Error => Err("FMU returned Error".into()),
        binding::fmi3Status_fmi3Fatal => {
            Err("FMU returned Fatal; no further calls to this model are permitted".into())
        }
        _ => Err(format!("FMU returned an invalid FMI status {value}")),
    }
}

unsafe extern "C" fn log_message(
    _: binding::fmi3InstanceEnvironment,
    status: binding::fmi3Status,
    category: binding::fmi3String,
    message: binding::fmi3String,
) {
    // FMI owns the strings for this callback's duration. An invalid null string
    // is diagnosed without dereferencing it or unwinding through the C ABI.
    if category.is_null() || message.is_null() {
        bevy::log::error!("[fmi] log callback supplied a null message/category");
        return;
    }
    let category = unsafe { CStr::from_ptr(category) }.to_string_lossy();
    let message = unsafe { CStr::from_ptr(message) }.to_string_lossy();
    bevy::log::warn!("[fmi] status={status} category={category}: {message}");
}

impl Session {
    pub(crate) fn load(
        bytes: &[u8],
        name: &str,
        initial_inputs: &Values,
        parameters: &Values,
        communication_step: f64,
        max_unpacked_bytes: u64,
        max_event_iterations: usize,
        fatal_registry: &mut FatalRegistry,
    ) -> Result<(Self, Description), String> {
        if !communication_step.is_finite() || communication_step <= 0.0 || max_event_iterations == 0
        {
            return Err(
                "FMU admission requires a positive interval and event-iteration bound".into(),
            );
        }
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
        let mut unpacked = 0_u64;
        for index in 0..archive.len() {
            let member = archive.by_index(index).map_err(|e| e.to_string())?;
            if member.enclosed_name().is_none() || member.is_symlink() {
                return Err("FMU archive member must have an enclosed non-symlink identity".into());
            }
            unpacked = unpacked
                .checked_add(member.size())
                .ok_or("FMU size overflows")?;
            if unpacked > max_unpacked_bytes {
                return Err("FMU exceeds unpacked-byte budget".into());
            }
        }
        let resources = tempfile::Builder::new()
            .prefix("lunco-fmu-")
            .tempdir()
            .map_err(|e| e.to_string())?;
        archive
            .extract(resources.path())
            .map_err(|e| e.to_string())?;
        let source = std::fs::read_to_string(resources.path().join("modelDescription.xml"))
            .map_err(|e| e.to_string())?;
        let document = roxmltree::Document::parse(&source).map_err(|e| e.to_string())?;
        let md =
            ModelDescription::from_node(&document.root_element()).map_err(|e| e.to_string())?;
        if md.fmiVersion != "3.0" {
            return Err(format!(
                "unsupported FMI version '{}'; native import requires FMI 3.0",
                md.fmiVersion
            ));
        }
        let problems = md.validate();
        if !problems.is_empty() {
            return Err(format!(
                "invalid FMI model description: {}",
                problems
                    .iter()
                    .take(8)
                    .map(|problem| problem.message.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        let time_unit = super::metadata::time_unit(&md, &document)?;
        let native_start = time_unit.from_si(0.0).map_err(|e| e.to_string())?;
        let communication_step = communication_step / time_unit.scale_to_si();
        if !communication_step.is_finite() || communication_step <= 0.0 {
            return Err("FMU communication interval cannot be represented in its time unit".into());
        }
        let cs = md
            .coSimulation
            .as_ref()
            .ok_or("FMU must declare FMI 3 CoSimulation")?;
        if cs.needsExecutionTool {
            return Err("FMU requires an external execution tool".into());
        }
        if cs.canBeInstantiatedOnlyOncePerProcess {
            return Err("single-instance-per-process FMUs require a process-isolated host".into());
        }
        if let Some(internal) = &cs.fixedInternalStepSize {
            let internal = internal
                .parse::<f64>()
                .map_err(|e| format!("invalid fixedInternalStepSize: {e}"))?;
            let ratio = communication_step / internal;
            if !internal.is_finite()
                || internal <= 0.0
                || !ratio.is_finite()
                || ratio.round() < 1.0
                || (ratio - ratio.round()).abs() > 32.0 * f64::EPSILON * ratio.abs().max(1.0)
            {
                return Err(
                    "communication period must be an integer multiple of fixedInternalStepSize"
                        .into(),
                );
            }
        }
        let mut inputs = BTreeMap::new();
        let mut outputs = BTreeMap::new();
        let mut params = BTreeMap::new();
        let mut contracts = lunco_port_core::ScalarPortContracts::default();
        let nodes: BTreeMap<_, _> = document
            .root_element()
            .children()
            .find(|node| node.has_tag_name("ModelVariables"))
            .ok_or("missing FMI ModelVariables")?
            .children()
            .filter(|node| node.is_element())
            .filter_map(|node| node.attribute("name").map(|name| (name, node)))
            .collect();
        for variable in &md.modelVariables {
            if matches!(variable.variableType, VariableType::Clock { .. }) {
                return Err(
                    "clocked FMUs require a Scheduled Execution/clock activation host".into(),
                );
            }
            if matches!(
                variable.causality,
                Causality::Input | Causality::Output | Causality::Parameter
            ) {
                if !matches!(variable.variableType, VariableType::Float64 { .. }) {
                    if matches!(variable.causality, Causality::Input | Causality::Output) {
                        return Err(format!(
                            "FMU port '{}' cannot use the scalar Float64 wire contract",
                            variable.name
                        ));
                    }
                    continue;
                }
                if !variable.dimensions.is_empty() || !variable.clocks.is_empty() {
                    return Err(format!(
                        "FMU port '{}' must be an unclocked scalar Float64",
                        variable.name
                    ));
                }
                if variable.name.is_empty() || variable.name.contains('\0') {
                    return Err("FMU variable has an invalid name".into());
                }
                let node = *nodes
                    .get(variable.name.as_str())
                    .ok_or("missing FMI variable source node")?;
                contracts.set(
                    variable.name.clone(),
                    super::metadata::scalar_contract(&md, variable, node)?,
                )?;
                let map = match variable.causality {
                    Causality::Input => &mut inputs,
                    Causality::Output => &mut outputs,
                    _ => &mut params,
                };
                if map
                    .insert(variable.name.clone(), variable.valueReference)
                    .is_some()
                {
                    return Err("FMU variable names must be unique".into());
                }
            }
        }
        let mut dependencies = BTreeMap::new();
        for (name, vr) in &outputs {
            let unknown = md
                .outputs
                .iter()
                .find(|item| item.valueReference == *vr)
                .ok_or_else(|| format!("FMU output '{name}' has no ModelStructure entry"))?;
            // Omitted dependencies conservatively include all inputs; an
            // explicit empty list declares no direct input feedthrough.
            dependencies.insert(
                name.clone(),
                inputs
                    .iter()
                    .filter(|(_, input)| {
                        unknown
                            .dependencies
                            .as_ref()
                            .is_none_or(|list| list.contains(input))
                    })
                    .map(|(name, _)| name.clone())
                    .collect(),
            );
        }
        let event_mode = cs.hasEventMode;
        let variable_step = cs.canHandleVariableCommunicationStepSize;
        let rollback = cs.canGetAndSetFMUState;
        let fatal = fatal_registry
            .entry((md.instantiationToken.clone(), cs.modelIdentifier.clone()))
            .or_default()
            .clone();
        if fatal.get() {
            return Err("FMU model is unavailable after a fatal native failure".into());
        }
        // modelIdentifier names a platform library, never a path outside the
        // admitted archive. This is the FMI C-identifier grammar.
        let mut identifier = cs.modelIdentifier.bytes();
        if !identifier
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
            || !identifier.all(|c| c.is_ascii_alphanumeric() || c == b'_')
        {
            return Err("FMU modelIdentifier must be a C identifier".into());
        }
        let path = resources
            .path()
            .join("binaries")
            .join(fmi_rs::fmi3::native::PLATFORM_TUPLE)
            .join(format!(
                "{}{}",
                cs.modelIdentifier,
                std::env::consts::DLL_SUFFIX
            ));
        let library = unsafe { binding::Fmi3Binding::new(path) }.map_err(|e| e.to_string())?;
        let instance_name = CString::new(name).map_err(|e| e.to_string())?;
        let token = CString::new(md.instantiationToken.as_bytes()).map_err(|e| e.to_string())?;
        // FMI 3 resourcePath is an absolute native directory with a trailing
        // separator. Keep it UTF-8 at the explicit native ABI boundary.
        let mut resource_path = resources.path().join("resources");
        resource_path.push("");
        let resource_path = resource_path
            .to_str()
            .ok_or("FMU resource path is not UTF-8")?;
        let resource_path = CString::new(resource_path).map_err(|e| e.to_string())?;
        let instance = unsafe {
            library.fmi3InstantiateCoSimulation(
                instance_name.as_ptr(),
                token.as_ptr(),
                resource_path.as_ptr(),
                false,
                false,
                event_mode,
                true,
                ptr::null(),
                0,
                ptr::null_mut(),
                Some(log_message),
                None,
            )
        };
        if instance.is_null() {
            return Err("FMU instantiation failed".into());
        }
        let model_name = md.modelName.clone();
        let mut session = Self {
            binding: library,
            instance,
            _resources: resources,
            inputs,
            outputs,
            event_mode,
            variable_step,
            next_event_time: None,
            input_values: BTreeMap::new(),
            time: native_start,
            communication_step,
            time_unit,
            max_event_iterations,
            fatal,
            contracts: contracts.clone(),
        };
        // Fixed parameters are set in Instantiated Mode, before initialization.
        session.set_named(&params, parameters)?;
        session.check(unsafe {
            session.binding.fmi3EnterInitializationMode(
                session.instance,
                false,
                0.0,
                native_start,
                false,
                0.0,
            )
        })?;
        session.set_named(&session.inputs.clone(), initial_inputs)?;
        session.check(unsafe { session.binding.fmi3ExitInitializationMode(session.instance) })?;
        if event_mode {
            session.finish_event()?;
        }
        let initial = session.read_named(&session.inputs.clone())?;
        session.input_values = initial.iter().cloned().collect();
        let result = session.read_outputs()?;
        Ok((
            session,
            Description {
                name: model_name,
                inputs: initial,
                outputs: result,
                dependencies,
                rollback,
                contracts,
            },
        ))
    }

    fn check(&self, value: binding::fmi3Status) -> Result<(), String> {
        if value == binding::fmi3Status_fmi3Fatal {
            // FMI Fatal invalidates every instance of this model. Even
            // FreeInstance is forbidden afterward; the worker shares this
            // terminal flag across the model's token/implementation identity.
            self.fatal.set(true);
        }
        status(value)
    }

    fn set_named(&mut self, names: &BTreeMap<String, u32>, values: &Values) -> Result<(), String> {
        let mut refs = Vec::with_capacity(values.len());
        let mut data = Vec::with_capacity(values.len());
        let mut seen = std::collections::BTreeSet::new();
        for (name, value) in values {
            if !value.is_finite() || !seen.insert(name) {
                return Err("FMU input batch must contain unique finite values".into());
            }
            self.contracts
                .get(name)
                .ok_or_else(|| format!("FMU scalar '{name}' has no admitted contract"))?
                .metadata(
                    lunco_port_core::ports::PortDirection::In,
                    "FMI",
                    "FMU declaration",
                )
                .validate(*value)
                .map_err(|error| format!("FMU scalar '{name}': {error}"))?;
            refs.push(
                *names
                    .get(name)
                    .ok_or_else(|| format!("FMU has no writable scalar '{name}'"))?,
            );
            data.push(*value);
        }
        if !refs.is_empty() {
            self.check(unsafe {
                self.binding.fmi3SetFloat64(
                    self.instance,
                    refs.as_ptr(),
                    refs.len(),
                    data.as_ptr(),
                    data.len(),
                )
            })?;
        }
        Ok(())
    }

    fn read_named(&mut self, names: &BTreeMap<String, u32>) -> Result<Values, String> {
        let refs: Vec<_> = names.values().copied().collect();
        let mut data = vec![0.0; refs.len()];
        if !refs.is_empty() {
            self.check(unsafe {
                self.binding.fmi3GetFloat64(
                    self.instance,
                    refs.as_ptr(),
                    refs.len(),
                    data.as_mut_ptr(),
                    data.len(),
                )
            })?;
        }
        if data.iter().any(|value| !value.is_finite()) {
            return Err("FMU returned a non-finite sample".into());
        }
        for (name, value) in names.keys().zip(&data) {
            self.contracts
                .get(name)
                .ok_or_else(|| format!("FMU scalar '{name}' has no admitted contract"))?
                .metadata(
                    lunco_port_core::ports::PortDirection::In,
                    "FMI",
                    "FMU declaration",
                )
                .validate(*value)
                .map_err(|error| format!("FMU scalar '{name}': {error}"))?;
        }
        Ok(names.keys().cloned().zip(data).collect())
    }

    fn read_outputs(&mut self) -> Result<Values, String> {
        self.read_named(&self.outputs.clone())
    }

    fn finish_event(&mut self) -> Result<(), String> {
        for _ in 0..self.max_event_iterations {
            let (mut more, mut terminate, mut nominals, mut states, mut next_defined, mut next) =
                (false, false, false, false, false, 0.0);
            self.check(unsafe {
                self.binding.fmi3UpdateDiscreteStates(
                    self.instance,
                    &mut more,
                    &mut terminate,
                    &mut nominals,
                    &mut states,
                    &mut next_defined,
                    &mut next,
                )
            })?;
            if terminate {
                self.check(unsafe { self.binding.fmi3Terminate(self.instance) })?;
                return Err("FMU requested termination during event iteration".into());
            }
            if more {
                continue;
            }
            self.next_event_time = if next_defined {
                if !next.is_finite() || next <= self.time {
                    return Err(
                        "FMU nextEventTime must be finite and later than its current time".into(),
                    );
                }
                Some(next)
            } else {
                None
            };
            return self.check(unsafe { self.binding.fmi3EnterStepMode(self.instance) });
        }
        Err("FMU event iteration exceeded its configured bound".into())
    }

    pub(crate) fn time(&self) -> Result<f64, String> {
        self.time_unit.to_si(self.time).map_err(|e| e.to_string())
    }

    pub(crate) fn step(
        &mut self,
        start: f64,
        stop: f64,
        inputs: &Values,
    ) -> Result<Values, String> {
        let start = self.time_unit.from_si(start).map_err(|e| e.to_string())?;
        let stop = self.time_unit.from_si(stop).map_err(|e| e.to_string())?;
        if self.fatal.get() {
            return Err("FMU model is unavailable after a fatal native failure".into());
        }
        if !start.is_finite()
            || !stop.is_finite()
            || stop <= start
            || !close(start, self.time)
            || !close(stop - start, self.communication_step)
        {
            return Err("FMU step does not match its admitted communication interval".into());
        }
        let mut seen = std::collections::BTreeSet::new();
        let mut changed = Vec::new();
        for (name, value) in inputs {
            if !value.is_finite() || !seen.insert(name) || !self.inputs.contains_key(name) {
                return Err("FMU input transaction requires unique declared finite samples".into());
            }
            if self.input_values.get(name) != Some(value) {
                changed.push((name.clone(), *value));
            }
        }
        // A held input change is a discontinuity. If Event Mode is in use,
        // perform that transaction there before returning to Step Mode.
        if !changed.is_empty() {
            if self.event_mode {
                self.check(unsafe { self.binding.fmi3EnterEventMode(self.instance) })?;
            }
            self.set_named(&self.inputs.clone(), &changed)?;
            if self.event_mode {
                self.finish_event()?;
            }
            self.input_values.extend(changed);
        }
        // Step Mode outputs are computed by doStep, not by set/get iteration.
        // Early returns and events stay private until the full endpoint commits.
        // The grid defines the requested horizon. Each native call resumes
        // at the preceding lastSuccessfulTime to preserve the FMU's actual clock.
        let mut current = self.time;
        for _ in 0..self.max_event_iterations {
            let (mut event, mut terminate, mut early, mut last) = (false, false, false, f64::NAN);
            let endpoint = self
                .next_event_time
                .filter(|time| *time < stop)
                .unwrap_or(stop);
            if endpoint <= current {
                return Err("FMU pending time event is not after the current point".into());
            }
            let duration = if !self.variable_step && close(current, start) && endpoint == stop {
                self.communication_step
            } else {
                endpoint - current
            };
            if !self.variable_step && !close(duration, self.communication_step) {
                return Err("FMU cannot handle the variable interval required by its early return/time event".into());
            }
            self.check(unsafe {
                self.binding.fmi3DoStep(
                    self.instance,
                    current,
                    duration,
                    true,
                    &mut event,
                    &mut terminate,
                    &mut early,
                    &mut last,
                )
            })?;
            if terminate {
                self.check(unsafe { self.binding.fmi3Terminate(self.instance) })?;
                return Err("FMU requested simulation termination".into());
            }
            if !last.is_finite() || last < current || (last > endpoint && !close(last, endpoint)) {
                return Err("FMU returned an invalid lastSuccessfulTime".into());
            }
            self.time = last;
            let time_event = self
                .next_event_time
                .is_some_and(|time| last >= time || close(last, time));
            if event || time_event {
                if !self.event_mode {
                    return Err("FMU requested an undeclared Event Mode".into());
                }
                self.check(unsafe { self.binding.fmi3EnterEventMode(self.instance) })?;
                self.finish_event()?;
            }
            if close(last, stop) {
                return self.read_outputs();
            }
            if (!early && !close(last, endpoint))
                || last >= stop
                || (last == current && !event && !time_event)
            {
                return Err("FMU early return did not make valid time/event progress".into());
            }
            current = last;
        }
        Err("FMU communication interval exceeded its event/early-return bound".into())
    }
}

fn close(first: f64, second: f64) -> bool {
    first.is_finite()
        && second.is_finite()
        && (first - second).abs() <= 32.0 * f64::EPSILON * first.abs().max(second.abs()).max(1.0)
}

impl Drop for Session {
    fn drop(&mut self) {
        // Extracted resources outlive the native instance.
        if !self.fatal.get() {
            unsafe {
                self.binding.fmi3FreeInstance(self.instance);
            }
        }
    }
}
