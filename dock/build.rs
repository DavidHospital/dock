use std::{
    fs::{self},
    io,
    path::{Path, PathBuf},
};

const SHADERS_DIR: &str = "src/shaders";

fn main() -> anyhow::Result<()> {
    // println!("cargo::rerun-if-changed={SHADERS_DIR}");

    for path in walk_dir(SHADERS_DIR)? {
        let _ = transpile_shader_file(&path).inspect_err(|err| eprintln!("{err}"));
    }

    Ok(())
}

fn transpile_shader_file<P>(path: P) -> anyhow::Result<()>
where
    P: AsRef<Path>,
{
    let path = path.as_ref();
    if path.extension().is_none_or(|ext| ext != "wgsl") {
        return Ok(());
    }

    let Some(vert_out_path) = output_file_name(path, "vert") else {
        return Ok(());
    };
    let Some(frag_out_path) = output_file_name(path, "frag") else {
        return Ok(());
    };

    let wgsl_code = fs::read_to_string(path)?;

    let module = naga::front::wgsl::parse_str(&wgsl_code)?;
    let module_info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .subgroup_stages(naga::valid::ShaderStages::all())
    .subgroup_operations(naga::valid::SubgroupOperationSet::all())
    .validate(&module)?;

    let mut writer = naga::back::spv::Writer::new(&Default::default())?;
    let mut vert_out = Vec::<u32>::new();
    let mut frag_out = Vec::<u32>::new();
    writer.write(
        &module,
        &module_info,
        Some(&naga::back::spv::PipelineOptions {
            shader_stage: naga::ShaderStage::Vertex,
            entry_point: "vs_main".to_string(),
        }),
        &None,
        &mut vert_out,
    )?;
    writer.write(
        &module,
        &module_info,
        Some(&naga::back::spv::PipelineOptions {
            shader_stage: naga::ShaderStage::Fragment,
            entry_point: "fs_main".to_string(),
        }),
        &None,
        &mut frag_out,
    )?;

    fs::write(&vert_out_path, bytemuck::cast_slice(&vert_out))?;
    fs::write(&frag_out_path, bytemuck::cast_slice(&frag_out))?;

    Ok(())
}

fn walk_dir<P>(path: P) -> io::Result<Vec<PathBuf>>
where
    P: AsRef<Path>,
{
    let path = path.as_ref();
    if !path.is_dir() {
        return Ok(vec![path.to_path_buf()]);
    }

    let files = fs::read_dir(path)?
        .filter_map(Result::ok)
        .flat_map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                walk_dir(&path)
            } else {
                Ok(vec![path.to_path_buf()])
            }
        })
        .flatten()
        .collect();

    Ok(files)
}

fn output_file_name<P>(wgsl_path: P, kind: &str) -> Option<String>
where
    P: AsRef<Path>,
{
    let wgsl_path = wgsl_path.as_ref();
    let prefix = wgsl_path.file_prefix()?.to_str()?;
    Some(format!("{SHADERS_DIR}/{prefix}.{kind}.spv"))
}
