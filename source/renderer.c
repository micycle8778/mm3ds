#include <stddef.h>
#include <stdlib.h>

#include <3ds.h>
#include <citro3d.h>
#include <tex3ds.h>

#include "inc/panic.h"
#include "inc/renderer.h"

#include "vshader_shbin.h"

struct renderer*
renderer_init() {
    struct renderer* ret = malloc(sizeof(*ret)); //calloc(1, sizeof(struct renderer));
    PANIC_IF_NULL(ret);

    Mtx_PerspTilt(&ret->projection, C3D_AngleFromDegrees(80.0f), C3D_AspectRatioTop, 0.01f, 1000.0f, false);

    ret->shader_dvlb = DVLB_ParseFile((u32*)vshader_shbin, vshader_shbin_size);
    shaderProgramInit(&ret->shader_program);
    shaderProgramSetVsh(&ret->shader_program, &ret->shader_dvlb->DVLE[0]);
    C3D_BindProgram(&ret->shader_program);

    //#define HORSE(x) printf(#x ": %d", x);
    ret->uLoc_projection   = shaderInstanceGetUniformLocation(ret->shader_program.vertexShader, "projection");
    ret->uLoc_modelView    = shaderInstanceGetUniformLocation(ret->shader_program.vertexShader, "modelView");
    ret->uLoc_lightVec     = shaderInstanceGetUniformLocation(ret->shader_program.vertexShader, "lightVec");
    ret->uLoc_lightHalfVec = shaderInstanceGetUniformLocation(ret->shader_program.vertexShader, "lightHalfVec");
    ret->uLoc_lightClr     = shaderInstanceGetUniformLocation(ret->shader_program.vertexShader, "lightClr");
    ret->uLoc_material     = shaderInstanceGetUniformLocation(ret->shader_program.vertexShader, "material");
    // HORSE(ret->uLoc_projection);
    // HORSE(ret->uLoc_modelView);
    // HORSE(ret->uLoc_lightVec);
    // HORSE(ret->uLoc_lightHalfVec);
    // HORSE(ret->uLoc_lightClr);
    // HORSE(ret->uLoc_material);

    ret->attr_info = C3D_GetAttrInfo();
    AttrInfo_Init(ret->attr_info);
    AttrInfo_AddLoader(ret->attr_info, 0, GPU_FLOAT, 3); // v0=position
    AttrInfo_AddLoader(ret->attr_info, 1, GPU_FLOAT, 2); // v1=texcoord
    AttrInfo_AddLoader(ret->attr_info, 2, GPU_FLOAT, 3); // v2=normal

    // Configure the first fragment shading substage to blend the texture color with
    // the vertex color (calculated by the vertex shader using a lighting algorithm)
    // See https://www.opengl.org/sdk/docs/man2/xhtml/glTexEnv.xml for more insight
    C3D_TexEnv* env = C3D_GetTexEnv(0);
    C3D_TexEnvInit(env);
    C3D_TexEnvSrc(env, C3D_Both, GPU_TEXTURE0, GPU_PRIMARY_COLOR, 0);
    C3D_TexEnvFunc(env, C3D_Both, GPU_MODULATE);

    return ret;
}

static size_t 
new_mesh(struct renderer* this) {
    PANIC_IF_NULL(this);

    if (this->meshes == NULL) {
        this->meshes = malloc(10 * sizeof(struct mesh));
        if (this->meshes == NULL) PANIC("failed malloc");
        this->capacity_meshes = 10;
    }
    else if (this->n_meshes == this->capacity_meshes) {
        size_t newcap = (this->capacity_meshes * 3) / 2;
        this->meshes = realloc(this->meshes, newcap);
        if (this->meshes == NULL) PANIC("failed realloc");
        this->capacity_meshes = newcap;
    }

    this->n_meshes += 1;
    return this->n_meshes - 1;
}

// registers a mesh into the renderer
//
// returns mesh id on success
size_t
renderer_register_mesh(
        struct renderer* this,

        const struct vertex* vertices,
        size_t n_vertices,

        const void* texture_data,
        size_t texture_size,

        struct material material
) {
    PANIC_IF_NULL(this);
    PANIC_IF_NULL(vertices);
    PANIC_IF_NULL(texture_data);
    PANIC_IF_ZERO(n_vertices);
    PANIC_IF_ZERO(texture_size);
    
    size_t ret = new_mesh(this);
    struct mesh* mesh = &this->meshes[ret];

    // HORSE CHANGE
    mesh->material = material;

    size_t size = sizeof(struct vertex) * n_vertices;
    mesh->vbo_data = linearAlloc(size);
    memcpy(mesh->vbo_data, vertices, size);
    mesh->vertex_count = n_vertices;
    PANIC_IF_NULL(mesh->vbo_data);

    printf("mesh: #=%zu, sz=%zu, total=%zu\n", n_vertices, sizeof(struct vertex), size);
    printf("texture: %p\n", mesh->texture.data);

    Tex3DS_Texture t3x = Tex3DS_TextureImport(texture_data, texture_size, &mesh->texture, NULL, false);
    if (!t3x) {
        PANIC("importing t3x texture failed!");
    }
    // "Delete the t3x object since we don't need it."
    Tex3DS_TextureFree(t3x); 

    C3D_TexSetFilter(&mesh->texture, GPU_LINEAR, GPU_NEAREST);

    printf("texture: %p\n", mesh->texture.data);

    mesh->buf_info = C3D_GetBufInfo();
    BufInfo_Init(mesh->buf_info);
    /* HORSE_CHANGE: &mesh->vbo_data => mesh->vbo_data */
    BufInfo_Add(mesh->buf_info, mesh->vbo_data, sizeof(struct vertex), 3, 0x210);

    return ret;
}

void 
renderer_request(
        struct renderer* this,
        size_t mesh_id,
        const C3D_Mtx* model
) {
    if (this->requests == NULL) {
        this->requests = malloc(10 * sizeof(struct mesh));
        if (this->requests == NULL) PANIC("failed malloc");
        this->capacity_requests = 10;
    }
    else if (this->n_requests == this->capacity_requests) {
        size_t newcap = (this->capacity_requests * 3) / 2;
        this->meshes = realloc(this->requests, newcap);
        if (this->requests == NULL) PANIC("failed realloc");
        this->capacity_requests = newcap;
    }

    struct render_request* request = &this->requests[this->n_requests];
    this->n_requests += 1;

    request->mesh_id = mesh_id;
    request->model = *model;
}

void
renderer_render(struct renderer* this) {
    PANIC_IF_NULL(this);

    // C3D_BindProgram(&this->shader_program);
    for (size_t idx = 0; idx < this->n_requests; idx++) {
        struct mesh* mesh = &this->meshes[this->requests[idx].mesh_id];
        //C3D_Mtx *m = &this->requests[idx].model;
        C3D_Mtx *m = (C3D_Mtx*)&mesh->material;
        printf("mvp: %f %f %f %f\n", m->m[0], m->m[1], m->m[2], m->m[3]);
        printf("   : %f %f %f %f\n", m->m[0+4], m->m[1+4], m->m[2+4], m->m[3+4]);
        printf("   : %f %f %f %f\n", m->m[0+8], m->m[1+8], m->m[2+8], m->m[3+8]);
        printf("   : %f %f %f %f\n", m->m[0+12], m->m[1+12], m->m[2+12], m->m[3+12]);

        //printf("processing request %zu => %lu\n", idx, this->requests[idx].mesh_id);

        C3D_FVUnifMtx4x4(GPU_VERTEX_SHADER, this->uLoc_projection, &this->projection);
        C3D_FVUnifMtx4x4(GPU_VERTEX_SHADER, this->uLoc_modelView,  &this->requests[idx].model);
        C3D_FVUnifMtx4x4(GPU_VERTEX_SHADER, this->uLoc_material,   (C3D_Mtx*)&mesh->material);
        C3D_FVUnifSet(GPU_VERTEX_SHADER, this->uLoc_lightVec,     0.0f, 0.0f, -1.0f, 0.0f);
        C3D_FVUnifSet(GPU_VERTEX_SHADER, this->uLoc_lightHalfVec, 0.0f, 0.0f, -1.0f, 0.0f);
        C3D_FVUnifSet(GPU_VERTEX_SHADER, this->uLoc_lightClr,     1.0f, 1.0f,  1.0f, 1.0f);

        C3D_TexBind(0, &mesh->texture);  // bind texture
        //C3D_SetBufInfo(mesh->buf_info); // bind vertices
        C3D_DrawArrays(GPU_TRIANGLES, 0, mesh->vertex_count);
        //printf("mesh vc: %zu\n", mesh->vertex_count);
    }

    this->n_requests = 0;
}
