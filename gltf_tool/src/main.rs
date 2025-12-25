use std::io::Write;
use std::error::Error;
use std::fs::{self, File};
use std::io::BufWriter;
use std::process::Command;
use std::{env, iter};

use gltf::buffer;
use gltf::{Node, Primitive, mesh::Mode};
use gltf::image::{self, Source};

use glam::Vec4;
use glam::Vec4Swizzles;
use glam::{Mat4, Vec3, Vec3Swizzles};
use png::Encoder;

const TMP_PNG_FILENAME: &str = "gltftoolscratchspace.png";
const TMP_T3X_FILENAME: &str = "gltftoolscratchspace.t3x";

// this has to line up with what the engine expects
//
// position [f32; 3]
// uv [f32; 2]
// normal [f32; 3]
#[derive(Copy, Clone)]
#[repr(C)]
struct Vertex {
    pos: [f32; 3],
    uv: [f32; 2],
    normal: [f32; 3],
}

impl Vertex {
    fn bytes(self) -> impl Iterator<Item = u8> {
        self.pos.into_iter()
            .chain(self.uv)
            .chain(self.normal)
            .flat_map(|f| f.to_le_bytes())
    }
}

struct Mesh {
    vertices: Vec<Vertex>,
    color: Vec4,
    indices: Vec<u16>,
    texture: Option<Vec<u8>>,
}

fn work_with_nodes<'a>(
    nodes: impl IntoIterator<Item = Node<'a>>, 
    meshes: &mut Vec<Mesh>,
    buffers: &[buffer::Data],
) {
    for node in nodes {
        work_with_nodes(node.children(), meshes, buffers);

        if let Some(mesh) = node.mesh() {
            for prim in mesh.primitives() {
                if prim.mode() == Mode::Triangles {
                    let reader = prim.reader(|buf| Some(&buffers[buf.index()]));

                    let mat = prim.material();
                    let mut texture = None;
                    if let Some(tex_info) = mat.pbr_metallic_roughness().base_color_texture() {
                        let data = image::Data::from_source(
                            tex_info.texture().source().source(),
                            std::env::current_dir().ok().as_deref(), // TODO: change to path to file?
                            buffers
                        ).unwrap();

                        // now that we have the image data, lets save it as a png to a temporary
                        // file
                        
                        {
                            let mut writer = BufWriter::new(File::create_new(TMP_PNG_FILENAME).unwrap());
                            let mut encoder = Encoder::new(writer, data.width, data.height);
                            encoder.set_color(match data.format {
                                image::Format::R8G8B8
                                | image::Format::R16G16B16
                                => png::ColorType::Rgb,

                                image::Format::R8G8B8A8
                                | image::Format::R16G16B16A16
                                => png::ColorType::Rgba,

                                _ => todo!()
                            });
                            encoder.set_depth(match data.format {
                                image::Format::R8G8B8
                                | image::Format::R8G8B8A8
                                => png::BitDepth::Eight,

                                image::Format::R16G16B16
                                | image::Format::R16G16B16A16
                                => png::BitDepth::Sixteen,

                                _ => todo!()
                            });

                            let mut im_writer = encoder.write_header().unwrap();
                            im_writer.write_image_data(&data.pixels).unwrap();
                        }

                        // now lets tell tex3ds to convert that image into a t3x file

                        let status = Command::new("tex3ds")
                            .args("-f auto-etc1 -z auto".split_whitespace())
                            .args(["-o", TMP_T3X_FILENAME])
                            .arg(TMP_PNG_FILENAME)
                            .status()
                            .unwrap();
                        assert!(status.success());

                        // finally, embed the file into our mesh
                        texture = Some(fs::read(TMP_T3X_FILENAME).unwrap());

                        // and clean up
                        std::fs::remove_file(TMP_T3X_FILENAME).unwrap();
                        std::fs::remove_file(TMP_PNG_FILENAME).unwrap();
                    }
                    let it = reader.read_positions().unwrap()
                        .zip(reader.read_tex_coords(0).unwrap().into_f32())
                        .zip(reader.read_normals().unwrap())
                    ;

                    let mut vertices = Vec::with_capacity(it.len());
                    for ((pos, uv), normal) in it {
                        let pos = Mat4::from_cols_array_2d(&node.transform().matrix()) * Vec3::from(pos).xyzz().with_w(1.);
                        vertices.push(Vertex {
                            pos: pos.xyz().into(),
                            uv,
                            normal
                        });
                    }

                    let roughness = mat.pbr_metallic_roughness();

                    // dbg!(node.name().unwrap(), Vec4::from(roughness.base_color_factor()));
                    meshes.push(Mesh {
                        vertices,
                        color: roughness.base_color_factor().into(),
                        indices: reader.read_indices().unwrap().into_u32().map(|n| n.try_into().unwrap()).collect(),
                        texture
                    });
                }
            }
        }
    }
}

fn main() -> Result<(), Box<dyn Error>>{
    let (Some(in_file), Some(out_file)) = (env::args().nth(1), env::args().nth(2)) else {
        eprintln!("Usage: {} <input file> <output file>", env::args().next().unwrap());
        std::process::exit(1);
    };

    let (document, buffers, images) = gltf::import(in_file)?;
    let mut meshes: Vec<Mesh> = vec![];
    work_with_nodes(document.nodes(), &mut meshes, buffers.as_ref());

    let mut out_file = BufWriter::new(File::create(out_file)?);

    out_file.write_all(b"MESH")?;                                     // write file header
    out_file.write_all(&u32::try_from(meshes.len())?.to_le_bytes())?; // write the number of meshes

    let mut buf = Vec::new();
    for mesh in meshes {
        // write the color of this mesh
        buf.clear();
        let it = mesh.color.x.to_le_bytes().into_iter()
            .chain(mesh.color.y.to_le_bytes())
            .chain(mesh.color.z.to_le_bytes())
            .chain(mesh.color.w.to_le_bytes());
        buf.extend(it);
        out_file.write_all(&buf)?;

        out_file.write_all(&u32::try_from(mesh.vertices.len())?.to_le_bytes())?; // write number of vertices
        for vertex in mesh.vertices {
            // if this platform is little endian, this should be the same as doing
            // write(fd, (byte*)(&vertex), sizeof(vertex))
            buf.clear();
            buf.extend(vertex.bytes());
            out_file.write_all(&buf)?; // write the vertex
        }

        out_file.write_all(&u32::try_from(mesh.indices.len())?.to_le_bytes())?; // write number of indices
        for index in mesh.indices {
            // write the index
            out_file.write_all(&index.to_le_bytes())?;
        }

        if let Some(texture) = mesh.texture {
            out_file.write_all(&u32::try_from(texture.len())?.to_le_bytes())?; // write size of texture data
            out_file.write_all(&texture)?;
        } else {
            out_file.write_all(&[0; 4])?; // empty texture 
        }
    }
    
    Ok(())
}
