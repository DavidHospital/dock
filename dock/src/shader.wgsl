

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let x = f32(index & 1u) * 4.0 - 1.0;
    let y = f32((index >> 1u) & 1u) * 4.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) in: vec4<f32>) -> @location(0) vec4<f32> {
	// 0x192035
	let alpha = 0.65;
	let r = srgb_to_linear(0x19 / 255.0);
	let g = srgb_to_linear(0x20 / 255.0);
	let b = srgb_to_linear(0x35 / 255.0);
	let color = vec3<f32>(r, g, b);
	return vec4<f32>(color * alpha, alpha);
}

fn srgb_to_linear(c: f32) -> f32 {
	if c <= 0.04045 {
		return c / 12.92;
	}

	return pow((c + 0.055) / 1.055, 2.4);
}


