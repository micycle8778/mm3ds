#pragma once
#include <citro3d.h>

struct vertex {
    float position[3];
    float texcoord[2];
    float normal[3];
};

struct material {
    C3D_FVec ambient;
    C3D_FVec diffuse;
    C3D_FVec specular;
    C3D_FVec emission;
};

static const struct material default_material = {
    .ambient  = { {0.f, 0.2f, 0.2f, 0.2f} },
    .diffuse  = { {0.f, 0.2f, 0.2f, 0.2f} },
    .specular = { {0.f, 0.2f, 0.2f, 0.2f} },
    .emission = { {0.f, 0.2f, 0.2f, 0.2f} },
};

struct mesh {
    struct material material;
    struct vertex* vbo_data; // must be allocated in linear memory!
    C3D_BufInfo buf_info;
    C3D_Tex texture;
    size_t vertex_count;
};

struct render_request {
    u32 mesh_id; // id of the mesh
    C3D_Mtx model; // model matrix
};

struct renderer {
    // 8 byte align

    C3D_AttrInfo attr_info;

    // 4 byte align

    C3D_Mtx projection;

    DVLB_s* shader_dvlb;
    shaderProgram_s shader_program;

    int uLoc_projection;
    int uLoc_modelView;
    int uLoc_lightVec;
    int uLoc_lightHalfVec;
    int uLoc_lightClr;
    int uLoc_material;

    struct render_request* requests;
    size_t n_requests;
    size_t capacity_requests;

    struct mesh* meshes;
    size_t n_meshes;
    size_t capacity_meshes;
};

struct renderer* renderer_init();

size_t renderer_register_mesh(
        struct renderer* this,

        const struct vertex* vertices,
        size_t n_vertices,

        const void* texture_data,
        size_t texture_size,

        struct material material
);

void renderer_request(
        struct renderer* this,
        size_t mesh_id,
        const C3D_Mtx* model
);

void renderer_render(struct renderer* this);

