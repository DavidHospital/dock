use std::fs;

const WGSL_PATH_IN: &str = "src/shader.wgsl";
const SPV_VERT_PATH_OUT: &str = "src/shader.vert.spv";
const SPV_FRAG_PATH_OUT: &str = "src/shader.frag.spv";

fn main() -> anyhow::Result<()> {
    println!("cargo::rerun-if-changed=src/shader.wgsl");

    let wgsl_code = fs::read_to_string(&WGSL_PATH_IN)?;
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

    fs::write(&SPV_VERT_PATH_OUT, bytemuck::cast_slice(&vert_out))?;
    fs::write(&SPV_FRAG_PATH_OUT, bytemuck::cast_slice(&frag_out))?;

    Ok(())
}
