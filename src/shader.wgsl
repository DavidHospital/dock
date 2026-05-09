

@vertex
fn vs_main() -> @builtin(position) vec4<f32> {
	return vec4<f32>(0.);
}

@fragment
fn fs_main(@builtin(position) in: vec4<f32>) -> @location(0) vec4<f32> {
	return vec4<f32>(0.1, 0.2, 0.3, 1.0);
}


