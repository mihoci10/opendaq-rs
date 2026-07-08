// Draws the component tree of a reference device with box-drawing characters:
// every component with its type and local id, plus each component's visible
// properties with their type and current value.

use opendaq::{Component, CoreType, Folder, Instance, Property, PropertyObject, Value};

/// Readable type name for a component, e.g. "Channel" or "FunctionBlock".
fn type_label(component: &Component) -> String {
    component
        .component_kind()
        .map_or_else(|| "Component".to_string(), |kind| format!("{kind:?}"))
}

/// Printable value of `property` on `object`.  `property_value` already
/// returns scalars as native Rust values; structured ones come back as
/// wrappers and are shown as "<Type>" instead.
fn property_value_string(object: &PropertyObject, property: &Property) -> opendaq::Result<String> {
    Ok(match property.value_type()? {
        CoreType::Bool
        | CoreType::Int
        | CoreType::Float
        | CoreType::String
        | CoreType::Ratio
        | CoreType::ComplexNumber => match object.property_value(&property.name()?)? {
            Value::Str(text) => format!("{text:?}"),
            value => value.to_string(),
        },
        other => format!("<{other:?}>"),
    })
}

/// Print the visible properties of `component`, each line indented with `prefix`.
fn draw_properties(component: &Component, prefix: &str) -> opendaq::Result<()> {
    for property in component.visible_properties()? {
        println!(
            "{prefix}• {} : {:?} = {}",
            property.name()?,
            property.value_type()?,
            property_value_string(component, &property)?
        );
    }
    Ok(())
}

/// The immediate child components of `component` if it is a folder.
fn children(component: &Component) -> opendaq::Result<Vec<Component>> {
    if component.is_a::<Folder>() {
        component.cast::<Folder>()?.items()
    } else {
        Ok(Vec::new())
    }
}

fn draw_children(component: &Component, prefix: &str) -> opendaq::Result<()> {
    let kids = children(component)?;
    for (index, child) in kids.iter().enumerate() {
        let last = index + 1 == kids.len();
        let child_prefix = format!("{prefix}{}", if last { "   " } else { "│  " });
        println!(
            "{prefix}{}{} : {} ({})",
            if last { "└─ " } else { "├─ " },
            child.name()?,
            type_label(child),
            child.local_id()?
        );
        draw_properties(child, &child_prefix)?;
        draw_children(child, &child_prefix)?;
    }
    Ok(())
}

fn main() -> opendaq::Result<()> {
    let instance = Instance::new()?;
    instance.add_device("daqref://device0")?;

    let root = instance.root_device()?.expect("root device");
    println!(
        "{} : {} ({})",
        root.name()?,
        type_label(&root),
        root.local_id()?
    );
    draw_properties(&root, "")?;
    draw_children(&root, "")
}
