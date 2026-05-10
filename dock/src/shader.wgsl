
@group(0) @binding(0) var<storage, read> frame_bmask: array<u32>;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let x = f32(index & 1u) * 4.0 - 1.0;
    let y = f32((index >> 1u) & 1u) * 4.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) in: vec4<f32>) -> @location(0) vec4<f32> {
	let pos = vec2<u32>(in.xy);
	let index = (pos.y * 2560u + pos.x) / 32u;
	let bit_index = (pos.y * 2560u + pos.x) % 32u;
	let bit = (frame_bmask[index] >> bit_index) & 1u;

	// 0x192035
	let alpha = 0.65;
	let r = srgb_to_linear(0x19 / 255.0);
	let g = srgb_to_linear(0x20 / 255.0);
	let b = srgb_to_linear(0x35 / 255.0);
	let bg_color = vec3<f32>(r, g, b);

	// 0x7587BD
	let rf = srgb_to_linear(0x75 / 255.0);
	let gf = srgb_to_linear(0x87 / 255.0);
	let bf = srgb_to_linear(0xBD / 255.0);
	let fg_color = vec3<f32>(rf, gf, bf);
	let color = select(bg_color, fg_color, bool(bit));
	return vec4<f32>(color * alpha, alpha);
}

fn srgb_to_linear(c: f32) -> f32 {
	if c <= 0.04045 {
		return c / 12.92;
	}

	return pow((c + 0.055) / 1.055, 2.4);
}


