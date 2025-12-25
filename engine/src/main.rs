#![feature(allocator_api)]
use std::f32::consts::PI;
use std::io;
use std::io::Cursor;
use std::io::Read;
use std::marker::PhantomPinned;
use std::mem::MaybeUninit;
use std::pin::Pin;
use std::ptr;

use citro3d::attrib::Register;
use citro3d::buffer::Indices;
use citro3d::macros::include_shader;
use citro3d::math::AspectRatio;
use citro3d::math::ClipPlanes;
use citro3d::math::FVec4;
use citro3d::math::Projection;
use citro3d::render::DepthFormat;
use citro3d::render::Target;
use citro3d::shader;
use citro3d::sys;
use citro3d::buffer;
use citro3d::attrib::Format;
use citro3d::texenv;
use citro3d::render::ClearFlags;
use citro3d::uniform;
use citro3d::uniform::Uniform;
use citro3d::{Instance, attrib, math::Matrix4, shader::Program};
use ctru::services::gfx::RawFrameBuffer;
use ctru::services::gfx::TopScreen;
use ctru::{linear::LinearAllocator, prelude::*, set_panic_hook};
use ctru::services::gfx::Screen;
use glam::{Vec2, Vec3, Vec4, vec4, vec3, vec2};

#[derive(Copy, Clone)]
struct MeshId(usize);

#[derive(Copy, Clone)]
#[repr(C)]
struct Vertex {
    pos: Vec3,
    uv: Vec2,
    normal: Vec3
}

#[derive(Copy, Clone)]
#[repr(C)]
struct Material {
    ambient: FVec4,
    diffuse: FVec4,
    specular: FVec4,
    emission: FVec4,
}

impl From<Material> for Uniform {
    fn from(value: Material) -> Self {
        Matrix4::from_rows([
            value.ambient,
            value.diffuse,
            value.specular,
            value.emission,
        ]).into()
    }
}

impl Default for Material {
    fn default() -> Self {
        Self {
            ambient: vec4(0.2, 0.2, 0.2, 0.0).into(),
            diffuse: vec4(0.4, 0.4, 0.4, 0.0).into(),
            specular: vec4(0.8, 0.8, 0.8, 0.0).into(),
            emission: vec4(0.0, 0.0, 0.0, 1.0).into(),
        }
    }
}

struct Mesh {
    material: Material,
    vertices: Vec<Vertex, LinearAllocator>,
    texture: Option<sys::C3D_Tex>,
    buf_info: buffer::Info,

    vbo: Option<buffer::Slice<'static>>,
    indices: Option<Indices<'static, u16>>,

    _pinned: PhantomPinned
}

trait ReadExt {
    fn read_u16(&mut self) -> io::Result<u16>;
    fn read_u32(&mut self) -> io::Result<u32>;
    fn read_f32(&mut self) -> io::Result<f32>;
    fn read_vec2(&mut self) -> io::Result<Vec2>;
    fn read_vec3(&mut self) -> io::Result<Vec3>;
    fn read_vec4(&mut self) -> io::Result<Vec4>;
}

impl<T: Read> ReadExt for T {
    fn read_u16(&mut self) -> io::Result<u16> {
        let mut buf = [0u8; 2];
        self.read_exact(&mut buf)?;
        Ok(u16::from_le_bytes(buf))
    }

    fn read_u32(&mut self) -> io::Result<u32> {
        let mut buf = [0u8; 4];
        self.read_exact(&mut buf)?;
        Ok(u32::from_le_bytes(buf))
    }

    fn read_f32(&mut self) -> io::Result<f32> {
        let mut buf = [0u8; 4];
        self.read_exact(&mut buf)?;
        Ok(f32::from_le_bytes(buf))
    }

    fn read_vec2(&mut self) -> io::Result<Vec2> {
        Ok(Vec2::new(
            self.read_f32()?,
            self.read_f32()?,
        ))
    }

    fn read_vec3(&mut self) -> io::Result<Vec3> {
        Ok(Vec3::new(
            self.read_f32()?,
            self.read_f32()?,
            self.read_f32()?,
        ))
    }

    fn read_vec4(&mut self) -> io::Result<Vec4> {
        Ok(Vec4::new(
            self.read_f32()?,
            self.read_f32()?,
            self.read_f32()?,
            self.read_f32()?,
        ))
    }
}


impl Mesh {
    fn attr_info() -> attrib::Info {
        let mut ret = attrib::Info::new();
        ret.add_loader(Register::new(0).unwrap(), Format::Float, 3).unwrap(); // v0=position
        ret.add_loader(Register::new(1).unwrap(), Format::Float, 2).unwrap(); // v1=uv
        ret.add_loader(Register::new(2).unwrap(), Format::Float, 3).unwrap(); // v2=normal

        ret
    }

    fn from_file_data(mut reader: impl Read) -> io::Result<Vec<Pin<Box<Mesh>>>> {
        let mut magic = [0u8; 4];
        reader.read_exact(&mut magic)?;

        if magic != *b"MESH" {
            return Err(io::Error::other("invalid mesh file"));
        }

        let n_meshes = reader.read_u32()?;
        let mut ret = Vec::with_capacity(n_meshes as usize);
        for _ in 0..n_meshes {
            let material = Material {
                diffuse: reader.read_vec4()?.into(),
                ..Default::default()
            };

            let n_vertices = reader.read_u32()?;
            let mut vertices = Vec::with_capacity_in(n_vertices as usize, LinearAllocator);
            for _ in 0..n_vertices {
                vertices.push(Vertex {
                    pos: reader.read_vec3()?,
                    uv: reader.read_vec2()?,
                    normal: reader.read_vec3()?,
                });
            }

            let n_indices = reader.read_u32()?;
            let mut indices = Vec::with_capacity(n_indices as usize);
            for _ in 0..n_indices {
                indices.push(reader.read_u16()?);
            }

            let size_of_tex = reader.read_u32()?;
            let texture = if size_of_tex != 0 {
                let mut buf = vec![0u8; size_of_tex as usize];
                reader.read_exact(&mut buf)?;
                println!("found texture!");
                Some(buf)
            } else { None };
            
            ret.push(Mesh::from_data_prealloc(
                vertices,
                Some(indices).as_deref(),
                texture.as_deref(),
                material
            ));
        }

        Ok(ret)
    }

    fn from_data(vertices: &[Vertex], indices: Option<&[u16]>, t3x_data: Option<&[u8]>, material: Material) -> Pin<Box<Self>> {
        let mut vbo_data = Vec::with_capacity_in(vertices.len(), LinearAllocator);
        vbo_data.extend_from_slice(vertices);

        Self::from_data_prealloc(vbo_data, indices, t3x_data, material)
    }

    fn from_data_prealloc(vbo_data: Vec<Vertex, LinearAllocator>, indices: Option<&[u16]>, t3x_data: Option<&[u8]>, material: Material) -> Pin<Box<Self>> {
        let texture = t3x_data.map(|t3x_data| {
            let mut texture = MaybeUninit::<sys::C3D_Tex>::uninit();
            unsafe {
                let t3x = sys::Tex3DS_TextureImport(
                    t3x_data.as_ptr().cast(), 
                    t3x_data.len(), 
                    texture.as_mut_ptr(), 
                    ptr::null_mut(), 
                    false
                );

                assert_ne!(t3x, ptr::null_mut());
                // "Delete the t3x object since we don't need it."
                sys::Tex3DS_TextureFree(t3x);

                sys::C3D_TexSetFilter(texture.as_mut_ptr(), ctru_sys::GPU_LINEAR, ctru_sys::GPU_NEAREST);
            }

            unsafe { texture.assume_init() }
        });

        let mut mesh = Box::pin(Mesh {
            material,
            texture: texture,
            vertices: vbo_data,
            buf_info: buffer::Info::new(),
            vbo: None,
            indices: None,
            _pinned: PhantomPinned
        });

        unsafe {
            // we have fun lying to the borrow checker
            let ref_mesh: &mut Mesh = &mut *(Pin::get_unchecked_mut(mesh.as_mut()) as *mut _);
            let vbo = ref_mesh.buf_info.add(&mesh.vertices, &Mesh::attr_info()).unwrap();
            ref_mesh.vbo = Some(std::mem::transmute::<_, buffer::Slice<'static>>(vbo));
        }

        if let Some(indices) = indices {
            unsafe {
                let ref_mesh: &mut Mesh = &mut *(Pin::get_unchecked_mut(mesh.as_mut()) as *mut _);
                ref_mesh.indices = Some(std::mem::transmute(
                    mesh.vbo.as_ref().unwrap().index_buffer(indices).unwrap()
                ));
            }
        }

        mesh
    }
}

struct Request {
    mesh_id: MeshId,
    model: Matrix4
}

struct Renderer<'gfx> {
    context: Instance,

    target: Target<'gfx>,

    projection: Matrix4,
    u_loc_projection: uniform::Index,
    u_loc_model_view: uniform::Index,
    u_loc_light_vec: uniform::Index,
    u_loc_light_half_vec: uniform::Index,
    u_loc_light_color: uniform::Index,
    u_loc_material: uniform::Index,
    _shader_library: shader::Library, // pin, but not really?
    shader_program: Program,

    requests: Vec<Request>,
    meshes: Vec<Pin<Box<Mesh>>>
}

impl<'gfx> Renderer<'gfx> {
    fn new(gfx: &'gfx Gfx) -> Self {
        let context = Instance::new().unwrap();
        let mut top_screen = gfx.top_screen.borrow_mut();
        let RawFrameBuffer { width, height, .. } = top_screen.raw_framebuffer();
        let target = context.render_target(width, height, top_screen, Some(DepthFormat::Depth24Stencil8)).unwrap();

        let v_lib = shader::Library::from_bytes(include_shader!("shader.pica")).unwrap();
        let v_entry = v_lib.get(0).unwrap();
        let shader_program = shader::Program::new(v_entry).unwrap();

        let projection = Projection::perspective(80.0_f32.to_radians(), AspectRatio::TopScreen, ClipPlanes { near: 0.01, far: 100.0 });

        Self {
            context,

            target,

            projection: projection.into(),

            u_loc_projection: shader_program.get_uniform("projection").unwrap(),
            u_loc_model_view: shader_program.get_uniform("modelView").unwrap(),
            u_loc_light_vec: shader_program.get_uniform("lightVec").unwrap(),
            u_loc_light_half_vec: shader_program.get_uniform("lightHalfVec").unwrap(),
            u_loc_light_color: shader_program.get_uniform("lightClr").unwrap(),
            u_loc_material: shader_program.get_uniform("material").unwrap(),

            _shader_library: v_lib,
            shader_program,

            requests: vec![],
            meshes: vec![],
        }
    }

    fn register_mesh(&mut self, mesh: Pin<Box<Mesh>>) -> MeshId {
        self.meshes.push(mesh);
        MeshId(self.meshes.len() - 1)
    }

    fn please_render(&mut self, mesh_id: MeshId, model: Matrix4) {
        self.requests.push(Request { mesh_id, model });
    }

    fn render(&mut self) {
        self.context.render_frame_with(|mut pass| {
            pass.bind_program(&self.shader_program);

            unsafe { sys::C3D_AlphaTest(true, ctru_sys::GPU_GREATER, 0x10); }
            unsafe { sys::C3D_CullFace(ctru_sys::GPU_CULL_NONE); }

            const CLEAR_COLOR: u32 = 0x68b0d8ff;
            self.target.clear(ClearFlags::ALL, CLEAR_COLOR, 0);
            pass.select_render_target(&self.target).unwrap();

            pass.set_attr_info(&Mesh::attr_info());
            for request in &self.requests {
                let mesh = &self.meshes[request.mesh_id.0];

                let light_dir = vec4(0., 0., 1., 0.).normalize();
                pass.bind_vertex_uniform(self.u_loc_projection, self.projection);
                pass.bind_vertex_uniform(self.u_loc_model_view, request.model);
                pass.bind_vertex_uniform(self.u_loc_light_vec, light_dir);
                pass.bind_vertex_uniform(self.u_loc_light_half_vec, light_dir);
                pass.bind_vertex_uniform(self.u_loc_light_color, Vec4::ONE);
                pass.bind_vertex_uniform(self.u_loc_material, mesh.material);

                let stage0 = texenv::Stage::new(0).unwrap();
                if let Some(tex) = &mesh.texture {
                    pass.texenv(stage0)
                        .src(texenv::Mode::BOTH, texenv::Source::PrimaryColor, Some(texenv::Source::Texture0), None)
                        .func(texenv::Mode::BOTH, texenv::CombineFunc::Modulate);
                    unsafe { sys::C3D_TexBind(0, tex as *const _ as *mut _); }
                } else {
                    let stage0 = texenv::Stage::new(0).unwrap();
                    pass.texenv(stage0)
                        .src(texenv::Mode::BOTH, texenv::Source::PrimaryColor, None, None)
                        .func(texenv::Mode::BOTH, texenv::CombineFunc::Modulate);
                }


                if let Some(indices) = &mesh.indices {
                    pass.draw_elements(buffer::Primitive::Triangles, mesh.vbo.unwrap(), indices);
                } else {
                    pass.draw_arrays(buffer::Primitive::Triangles, mesh.vbo.unwrap());
                }
            }

            pass
        });

        self.requests.clear();
    }
}

fn main() {
    set_panic_hook(false);
    let apt = Apt::new().unwrap();
    let mut hid = Hid::new().unwrap();
    let gfx = Gfx::new().unwrap();
    let _console = Console::new(gfx.bottom_screen.borrow_mut());

    println!("Hello, World!");

    const VERTICES: [Vertex; 36] = [
        Vertex { pos: vec3(-0.5, -0.5,  0.5), uv: vec2(0., 0.), normal: vec3(0., 0.,  1.) },
        Vertex { pos: vec3( 0.5, -0.5,  0.5), uv: vec2(1., 0.), normal: vec3(0., 0.,  1.) },
        Vertex { pos: vec3( 0.5,  0.5,  0.5), uv: vec2(1., 1.), normal: vec3(0., 0.,  1.) },

        Vertex { pos: vec3( 0.5,  0.5,  0.5), uv: vec2(1., 1.), normal: vec3(0., 0.,  1.) },
        Vertex { pos: vec3(-0.5,  0.5,  0.5), uv: vec2(0., 1.), normal: vec3(0., 0.,  1.) },
        Vertex { pos: vec3(-0.5, -0.5,  0.5), uv: vec2(0., 0.), normal: vec3(0., 0.,  1.) },


        Vertex { pos: vec3(-0.5, -0.5, -0.5), uv: vec2(0., 0.), normal: vec3(0., 0., -1.) },
        Vertex { pos: vec3(-0.5,  0.5, -0.5), uv: vec2(1., 0.), normal: vec3(0., 0., -1.) },
        Vertex { pos: vec3( 0.5,  0.5, -0.5), uv: vec2(1., 1.), normal: vec3(0., 0., -1.) },

        Vertex { pos: vec3( 0.5,  0.5, -0.5), uv: vec2(1., 1.), normal: vec3(0., 0., -1.) },
        Vertex { pos: vec3( 0.5, -0.5, -0.5), uv: vec2(0., 1.), normal: vec3(0., 0., -1.) },
        Vertex { pos: vec3(-0.5, -0.5, -0.5), uv: vec2(0., 0.), normal: vec3(0., 0., -1.) },


        Vertex { pos: vec3( 0.5, -0.5, -0.5), uv: vec2(0., 0.), normal: vec3(-1., 0., 0.) },
        Vertex { pos: vec3( 0.5,  0.5, -0.5), uv: vec2(1., 0.), normal: vec3(-1., 0., 0.) },
        Vertex { pos: vec3( 0.5,  0.5,  0.5), uv: vec2(1., 1.), normal: vec3(-1., 0., 0.) },

        Vertex { pos: vec3( 0.5,  0.5,  0.5), uv: vec2(1., 1.), normal: vec3(-1., 0., 0.) },
        Vertex { pos: vec3( 0.5, -0.5,  0.5), uv: vec2(0., 1.), normal: vec3(-1., 0., 0.) },
        Vertex { pos: vec3( 0.5, -0.5, -0.5), uv: vec2(0., 0.), normal: vec3(-1., 0., 0.) },


        Vertex { pos: vec3(-0.5, -0.5, -0.5), uv: vec2(0., 0.), normal: vec3( 1., 0., 0.) },
        Vertex { pos: vec3(-0.5, -0.5,  0.5), uv: vec2(1., 0.), normal: vec3( 1., 0., 0.) },
        Vertex { pos: vec3(-0.5,  0.5,  0.5), uv: vec2(1., 1.), normal: vec3( 1., 0., 0.) },

        Vertex { pos: vec3(-0.5,  0.5,  0.5), uv: vec2(1., 1.), normal: vec3( 1., 0., 0.) },
        Vertex { pos: vec3(-0.5,  0.5, -0.5), uv: vec2(0., 1.), normal: vec3( 1., 0., 0.) },
        Vertex { pos: vec3(-0.5, -0.5, -0.5), uv: vec2(0., 0.), normal: vec3( 1., 0., 0.) },


        Vertex { pos: vec3(-0.5,  0.5, -0.5), uv: vec2(0., 0.), normal: vec3(0.,  1., 0.) },
        Vertex { pos: vec3(-0.5,  0.5,  0.5), uv: vec2(1., 0.), normal: vec3(0.,  1., 0.) },
        Vertex { pos: vec3( 0.5,  0.5,  0.5), uv: vec2(1., 1.), normal: vec3(0.,  1., 0.) },

        Vertex { pos: vec3( 0.5,  0.5,  0.5), uv: vec2(1., 1.), normal: vec3(0.,  1., 0.) },
        Vertex { pos: vec3( 0.5,  0.5, -0.5), uv: vec2(0., 1.), normal: vec3(0.,  1., 0.) },
        Vertex { pos: vec3(-0.5,  0.5, -0.5), uv: vec2(0., 0.), normal: vec3(0.,  1., 0.) },


        Vertex { pos: vec3(-0.5, -0.5, -0.5), uv: vec2(0., 0.), normal: vec3(0., -1., 0.) },
        Vertex { pos: vec3( 0.5, -0.5, -0.5), uv: vec2(1., 0.), normal: vec3(0., -1., 0.) },
        Vertex { pos: vec3( 0.5, -0.5,  0.5), uv: vec2(1., 1.), normal: vec3(0., -1., 0.) },

        Vertex { pos: vec3( 0.5, -0.5,  0.5), uv: vec2(1., 1.), normal: vec3(0., -1., 0.) },
        Vertex { pos: vec3(-0.5, -0.5,  0.5), uv: vec2(0., 1.), normal: vec3(0., -1., 0.) },
        Vertex { pos: vec3(-0.5, -0.5, -0.5), uv: vec2(0., 0.), normal: vec3(0., -1., 0.) },
    ];

    let mut renderer = Renderer::new(&gfx);
    let cube = renderer.register_mesh(Mesh::from_data(
            &VERTICES, 
            None,
            Some(include_bytes!(concat!(env!("OUT_DIR"), "/lemon.t3x"))), 
            Material::default()
    ));

    let character_ids = Mesh::from_file_data(Cursor::new(include_bytes!("../gfx/character.mesh"))).unwrap()
        .into_iter()
        .map(|mesh| renderer.register_mesh(mesh))
        .collect::<Vec<_>>();

    let mut angle_x = 0.0_f32;
    let mut angle_y = 0.0_f32;

    while apt.main_loop() {
        gfx.wait_for_vblank();

        hid.scan_input();
        if hid.keys_down().contains(KeyPad::SELECT) {
            break;
        }

        for (x, z) in [(0., -2.)] {
            let mut model = Matrix4::identity();
            model.rotate_x(angle_x);
            model.rotate_y(angle_y);
            model.translate(x, 0., z + angle_x.sin() * 0.5);

            renderer.please_render(cube, model);
        }

        for (x, z) in [(-1.5, -3.), (1.5, -3.)] {
            for &mesh_id in &character_ids {
                let mut model = Matrix4::identity();
                model.rotate_x(angle_x);
                model.rotate_y(angle_y);
                model.scale(0.3, 0.3, 0.3);
                model.translate(x, 0., z + angle_x.sin() * 0.5);

                renderer.please_render(mesh_id, model);
            }
        }

        angle_x += PI / 180.;
        angle_y += PI / 360.;


        renderer.render();
    }
}
