#![feature(allocator_api)]
use std::f32::consts::PI;
use std::marker::PhantomPinned;
use std::mem::MaybeUninit;
use std::pin::Pin;
use std::ptr;

use citro3d::attrib::Register;
use citro3d::macros::include_shader;
use citro3d::math::AspectRatio;
use citro3d::math::ClipPlanes;
use citro3d::math::FVec4;
use citro3d::math::Projection;
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
    texture: sys::C3D_Tex,
    buf_info: buffer::Info,
    vbo: Option<buffer::Slice<'static>>,
    _pinned: PhantomPinned
}

struct Request {
    mesh_id: MeshId,
    model: Matrix4
}

struct Renderer<'gfx> {
    context: Instance,

    attr_info: attrib::Info,

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
        let target = context.render_target(width, height, top_screen, None).unwrap();

        let v_lib = shader::Library::from_bytes(include_shader!("shader.pica")).unwrap();
        let v_entry = v_lib.get(0).unwrap();
        let shader_program = shader::Program::new(v_entry).unwrap();

        let projection = Projection::perspective(80.0_f32.to_radians(), AspectRatio::TopScreen, ClipPlanes { near: 0.01, far: 100.0 });

        let mut attr_info = attrib::Info::new();
        attr_info.add_loader(Register::new(0).unwrap(), Format::Float, 3).unwrap(); // v0=position
        attr_info.add_loader(Register::new(1).unwrap(), Format::Float, 2).unwrap(); // v1=uv
        attr_info.add_loader(Register::new(2).unwrap(), Format::Float, 3).unwrap(); // v2=normal

        Self {
            context,

            attr_info,

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

    fn register_mesh(&mut self, vertices: &[Vertex], t3x_data: &[u8], material: Material) -> MeshId {
        let mut vbo_data = Vec::with_capacity_in(vertices.len(), LinearAllocator);
        vbo_data.extend_from_slice(vertices);

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

        let mut mesh = Box::pin(Mesh {
            material,
            texture: unsafe { texture.assume_init() },
            vertices: vbo_data,
            buf_info: buffer::Info::new(),
            vbo: None,
            _pinned: PhantomPinned
        });

        unsafe {
            // we have fun lying to the borrow checker
            let ref_mesh: &mut Mesh = &mut *(Pin::get_unchecked_mut(mesh.as_mut()) as *mut _);
            let vbo = ref_mesh.buf_info.add(&mesh.vertices, &self.attr_info).unwrap();
            ref_mesh.vbo = Some(std::mem::transmute::<_, buffer::Slice<'static>>(vbo));
        }


        self.meshes.push(mesh);
        MeshId(self.meshes.len() - 1)
    }

    fn please_render(&mut self, mesh_id: MeshId, model: Matrix4) {
        self.requests.push(Request { mesh_id, model });
    }

    fn render(&mut self) {
        self.context.render_frame_with(|mut pass| {
            pass.bind_program(&self.shader_program);

            const CLEAR_COLOR: u32 = 0x68b0d8ff;
            self.target.clear(ClearFlags::ALL, CLEAR_COLOR, 0);
            pass.select_render_target(&self.target).unwrap();

            let stage0 = texenv::Stage::new(0).unwrap();
            pass.texenv(stage0)
                .src(texenv::Mode::BOTH, texenv::Source::PrimaryColor, Some(texenv::Source::Texture0), None)
                .func(texenv::Mode::BOTH, texenv::CombineFunc::Modulate);

            for request in &self.requests {
                let mesh = &self.meshes[request.mesh_id.0];

                pass.bind_vertex_uniform(self.u_loc_projection, self.projection);
                pass.bind_vertex_uniform(self.u_loc_model_view, request.model);
                pass.bind_vertex_uniform(self.u_loc_light_vec, vec4(0.0, -1.0, 0.0, 0.0));
                pass.bind_vertex_uniform(self.u_loc_light_half_vec, vec4(0.0, -1.0, 0.0, 0.0));
                pass.bind_vertex_uniform(self.u_loc_light_color, Vec4::ONE);
                pass.bind_vertex_uniform(self.u_loc_material, mesh.material);

                unsafe { sys::C3D_TexBind(0, &mesh.texture as *const _ as *mut _); }
                pass.set_attr_info(&self.attr_info);
                pass.draw_arrays(buffer::Primitive::Triangles, mesh.vbo.unwrap());
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
    let mesh_id = renderer.register_mesh(&VERTICES, include_bytes!("../kitten.t3x"), Material::default());

    let mut angle_x = 0.0_f32;
    let mut angle_y = 0.0_f32;

    while apt.main_loop() {
        gfx.wait_for_vblank();

        hid.scan_input();
        if hid.keys_down().contains(KeyPad::SELECT) {
            break;
        }

        let mut model = Matrix4::identity();
        model.rotate_x(angle_x);
        model.rotate_y(angle_y);
        model.translate(0., 0., -2.0 + angle_x.sin() * 0.5);

        angle_x += PI / 180.;
        angle_y += PI / 360.;

        renderer.please_render(mesh_id, model);
        renderer.render();
    }
}
