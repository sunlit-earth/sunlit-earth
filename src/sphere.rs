use bytemuck::{Pod, Zeroable};

/// A vertex with position, normal, and UV coordinates.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

impl Vertex {
    /// The vertex buffer layout descriptor for wgpu.
    pub fn buffer_layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: size_of::<Vertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                // position
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x3,
                },
                // normal
                wgpu::VertexAttribute {
                    offset: 12,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x3,
                },
                // uv
                wgpu::VertexAttribute {
                    offset: 24,
                    shader_location: 2,
                    format: wgpu::VertexFormat::Float32x2,
                },
            ],
        }
    }
}

/// Mesh data for a UV sphere.
pub struct SphereMesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

/// Generate a UV sphere with the given number of stacks (horizontal rings)
/// and sectors (vertical slices).
#[allow(clippy::many_single_char_names, clippy::cast_precision_loss)]
pub fn generate_uv_sphere(stacks: u32, sectors: u32) -> SphereMesh {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();

    let stacks_f = stacks as f32;
    let sectors_f = sectors as f32;

    // Generate vertices
    for i in 0..=stacks {
        let stack_angle =
            std::f32::consts::FRAC_PI_2 - (i as f32) * std::f32::consts::PI / stacks_f;
        let xy = stack_angle.cos();
        let y = stack_angle.sin();

        for j in 0..=sectors {
            let sector_angle = (j as f32) * 2.0 * std::f32::consts::PI / sectors_f;

            let x = xy * sector_angle.cos();
            let z = xy * sector_angle.sin();

            let u = j as f32 / sectors_f;
            let v = i as f32 / stacks_f;

            vertices.push(Vertex {
                position: [x, y, z],
                normal: [x, y, z], // For a unit sphere, position == normal
                uv: [u, v],
            });
        }
    }

    // Generate indices (two triangles per quad)
    for i in 0..stacks {
        for j in 0..sectors {
            let first = i * (sectors + 1) + j;
            let second = first + sectors + 1;

            // First triangle
            indices.push(first);
            indices.push(second);
            indices.push(first + 1);

            // Second triangle
            indices.push(first + 1);
            indices.push(second);
            indices.push(second + 1);
        }
    }

    SphereMesh { vertices, indices }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sphere_has_correct_vertex_count() {
        let mesh = generate_uv_sphere(4, 8);
        // (stacks + 1) * (sectors + 1) vertices
        assert_eq!(mesh.vertices.len(), 5 * 9);
    }

    #[test]
    fn sphere_has_correct_index_count() {
        let mesh = generate_uv_sphere(4, 8);
        // stacks * sectors * 6 indices (2 triangles per quad, 3 indices each)
        assert_eq!(mesh.indices.len(), 4 * 8 * 6);
    }

    #[test]
    fn all_vertices_on_unit_sphere() {
        let mesh = generate_uv_sphere(32, 64);
        for v in &mesh.vertices {
            let len =
                (v.position[0].powi(2) + v.position[1].powi(2) + v.position[2].powi(2)).sqrt();
            assert!(
                (len - 1.0).abs() < 1e-5,
                "Vertex at {:?} has length {len}, expected 1.0",
                v.position
            );
        }
    }

    #[test]
    fn normals_match_positions_for_unit_sphere() {
        let mesh = generate_uv_sphere(16, 32);
        for v in &mesh.vertices {
            assert_eq!(v.position, v.normal);
        }
    }

    #[test]
    fn uv_coordinates_in_range() {
        let mesh = generate_uv_sphere(16, 32);
        for v in &mesh.vertices {
            assert!(
                (0.0..=1.0).contains(&v.uv[0]) && (0.0..=1.0).contains(&v.uv[1]),
                "UV {:?} out of [0,1] range",
                v.uv
            );
        }
    }

    #[test]
    fn indices_in_range() {
        let mesh = generate_uv_sphere(16, 32);
        let vertex_count = mesh.vertices.len() as u32;
        for &idx in &mesh.indices {
            assert!(
                idx < vertex_count,
                "Index {idx} >= vertex count {vertex_count}"
            );
        }
    }

    #[test]
    fn vertex_layout_has_three_attributes() {
        let layout = Vertex::buffer_layout();
        assert_eq!(layout.attributes.len(), 3);
        assert_eq!(layout.array_stride, 32); // 3+3+2 floats = 8 * 4 = 32 bytes
    }
}
