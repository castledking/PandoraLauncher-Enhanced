use gpui::{prelude::*, *};
use gpui_component::StyledExt;
use image::{Frame, RgbaImage};
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone, Copy)]
struct Vec3 {
    x: f32,
    y: f32,
    z: f32,
}

impl std::ops::Add for Vec3 {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Vec3 {
            x: self.x + o.x,
            y: self.y + o.y,
            z: self.z + o.z,
        }
    }
}
impl std::ops::Sub for Vec3 {
    type Output = Self;
    fn sub(self, o: Self) -> Self {
        Vec3 {
            x: self.x - o.x,
            y: self.y - o.y,
            z: self.z - o.z,
        }
    }
}

impl Vec3 {
    fn cross(self, o: Self) -> Self {
        Vec3 {
            x: self.y * o.z - self.z * o.y,
            y: self.z * o.x - self.x * o.z,
            z: self.x * o.y - self.y * o.x,
        }
    }
    fn normalize(self) -> Self {
        let len = (self.x * self.x + self.y * self.y + self.z * self.z).sqrt();
        if len > 0.0 {
            Vec3 {
                x: self.x / len,
                y: self.y / len,
                z: self.z / len,
            }
        } else {
            self
        }
    }
    fn dot(self, o: Self) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
}

// 3D rotations
fn rotate_x(mut p: Vec3, angle: f32) -> Vec3 {
    let (s, c) = angle.sin_cos();
    let y = p.y * c - p.z * s;
    let z = p.y * s + p.z * c;
    p.y = y;
    p.z = z;
    p
}
fn rotate_y(mut p: Vec3, angle: f32) -> Vec3 {
    let (s, c) = angle.sin_cos();
    let x = p.x * c + p.z * s;
    let z = -p.x * s + p.z * c;
    p.x = x;
    p.z = z;
    p
}
fn rotate_z(mut p: Vec3, angle: f32) -> Vec3 {
    let (s, c) = angle.sin_cos();
    let x = p.x * c - p.y * s;
    let y = p.x * s + p.y * c;
    p.x = x;
    p.y = y;
    p
}

#[derive(Clone, Copy, PartialEq)]
enum Limb {
    Head,
    Body,
    RightArm,
    LeftArm,
    RightLeg,
    LeftLeg,
    Cape,
}

struct BodyPart {
    pos: Vec3,
    size: Vec3,
    pivot: Vec3,
    rot: Vec3,
    dims: (f32, f32, f32), // w, h, d
    limb: Limb,
    is_overlay: bool,
}

fn build_parts(slim: bool) -> Vec<BodyPart> {
    let mut p = Vec::new();
    let arm_w = if slim { 3.0 } else { 4.0 };
    let arm_rx = if slim { -7.0 } else { -8.0 }; // right arm pos X
    let arm_lx = 4.0; // left arm pos X

    let make = |w, h, d, px, py, pz, piv_x, piv_y, piv_z, limb: Limb, is_overlay: bool| BodyPart {
        pos: Vec3 { x: px, y: py, z: pz },
        size: Vec3 { x: w, y: h, z: d },
        pivot: Vec3 {
            x: piv_x,
            y: piv_y,
            z: piv_z,
        },
        rot: Vec3 { x: 0., y: 0., z: 0. },
        dims: (w, h, d),
        limb,
        is_overlay,
    };

    // Base layer - using proper Minecraft UV coordinates
    // Head: UV (8,8) in 64x64, or (8,0) in 64x32 equivalent
    p.push(make(8., 8., 8., -4., 24., -4., 0., 24., 0., Limb::Head, false)); // Head
    p.push(make(8., 12., 4., -4., 12., -2., 0., 12., 0., Limb::Body, false)); // Body
    p.push(make(arm_w, 12., 4., arm_rx, 12., -2., -6., 22., 0., Limb::RightArm, false)); // R-Arm
    p.push(make(arm_w, 12., 4., arm_lx, 12., -2., 6., 22., 0., Limb::LeftArm, false)); // L-Arm
    p.push(make(4., 12., 4., -4., 0., -2., -2., 12., 0., Limb::RightLeg, false)); // R-Leg
    p.push(make(4., 12., 4., 0., 0., -2., 2., 12., 0., Limb::LeftLeg, false)); // L-Leg

    // Overlay layer (slightly larger)
    let b = 0.5;
    let h_b = b / 2.0;
    p.push(make(8., 8., 8., -4. - h_b, 24. - h_b, -4. - h_b, 0., 24., 0., Limb::Head, true)); // Head
    p.push(make(8., 12., 4., -4. - h_b, 12. - h_b, -2. - h_b, 0., 12., 0., Limb::Body, true)); // Body
    p.push(make(
        arm_w,
        12.,
        4.,
        arm_rx - h_b,
        12. - h_b,
        -2. - h_b,
        -6.,
        22.,
        0.,
        Limb::RightArm,
        true,
    )); // R-Arm
    p.push(make(arm_w, 12., 4., arm_lx - h_b, 12. - h_b, -2. - h_b, 6., 22., 0., Limb::LeftArm, true)); // L-Arm
    p.push(make(4., 12., 4., -4. - h_b, 0. - h_b, -2. - h_b, -2., 12., 0., Limb::RightLeg, true)); // R-Leg
    p.push(make(4., 12., 4., 0. - h_b, 0. - h_b, -2. - h_b, 2., 12., 0., Limb::LeftLeg, true)); // L-Leg

    // Cape: 10x16x1
    p.push(make(10., 16., 1., -5., 8., -3., 0., 24., -2., Limb::Cape, false));

    p
}

fn get_uv_for_part(limb: Limb, is_overlay: bool) -> (f32, f32) {
    if is_overlay {
        match limb {
            Limb::Head => (32.0, 0.0),
            Limb::Body => (16.0, 32.0),
            Limb::RightArm => (40.0, 32.0),
            Limb::LeftArm => (48.0, 48.0),
            Limb::RightLeg => (0.0, 32.0),
            Limb::LeftLeg => (0.0, 48.0),
            Limb::Cape => (0.0, 0.0),
        }
    } else {
        match limb {
            Limb::Head => (0.0, 0.0),
            Limb::Body => (16.0, 16.0),
            Limb::RightArm => (40.0, 16.0),
            Limb::LeftArm => (32.0, 48.0),
            Limb::RightLeg => (0.0, 16.0),
            Limb::LeftLeg => (16.0, 48.0),
            Limb::Cape => (0.0, 0.0),
        }
    }
}

struct Triangle {
    v: [(Vec3, (f32, f32)); 3],
    normal: Vec3,
}

fn generate_triangles(part: &BodyPart, is_64x32: bool) -> Vec<Triangle> {
    let bloat = if part.is_overlay { 0.5 } else { 0.0 };
    let sw = part.size.x + bloat;
    let sh = part.size.y + bloat;
    let sd = part.size.z + bloat;

    let (base_u, base_v) = get_uv_for_part(part.limb, part.is_overlay);
    let mut u = base_u;
    let mut v = base_v;
    let w = part.dims.0;
    let h = part.dims.1;
    let d = part.dims.2;

    let is_left = match part.limb {
        Limb::LeftArm | Limb::LeftLeg => true,
        _ => false,
    };

    // Determine if we need to mirror for 64x32 format
    if is_64x32 && is_left && !part.is_overlay {
        match part.limb {
            Limb::LeftArm => {
                u = 40.0;
                v = 16.0;
            }
            Limb::LeftLeg => {
                u = 0.0;
                v = 16.0;
            }
            _ => {}
        }
    }
    let need_mirror = is_64x32 && is_left && !part.is_overlay;

    let mut tris = Vec::new();

    // 8 vertices of the box
    let mut p000 = part.pos;
    let mut p100 = part.pos + Vec3 { x: sw, y: 0., z: 0. };
    let mut p110 = part.pos + Vec3 { x: sw, y: sh, z: 0. };
    let mut p010 = part.pos + Vec3 { x: 0., y: sh, z: 0. };
    let mut p001 = part.pos + Vec3 { x: 0., y: 0., z: sd };
    let mut p101 = part.pos + Vec3 { x: sw, y: 0., z: sd };
    let mut p111 = part.pos + Vec3 { x: sw, y: sh, z: sd };
    let mut p011 = part.pos + Vec3 { x: 0., y: sh, z: sd };

    // Mirror vertex positions for left limbs in 64x32
    if need_mirror {
        let pivot_x = part.pivot.x;
        let mirror = |v: Vec3| -> Vec3 {
            Vec3 {
                x: 2.0 * pivot_x - v.x,
                y: v.y,
                z: v.z,
            }
        };
        p000 = mirror(p000);
        p100 = mirror(p100);
        p110 = mirror(p110);
        p010 = mirror(p010);
        p001 = mirror(p001);
        p101 = mirror(p101);
        p111 = mirror(p111);
        p011 = mirror(p011);
    }

    let mut add_face = |p0: Vec3, p1: Vec3, p2: Vec3, p3: Vec3, tx: f32, ty: f32, tw: f32, th: f32| {
        let (mut u0, mut v0) = (tx, ty);
        let (mut u1, mut v1) = (tx + tw, ty);
        let (mut u2, mut v2) = (tx + tw, ty + th);
        let (mut u3, mut v3) = (tx, ty + th);

        // Flip UVs for mirrored limbs
        if need_mirror {
            u0 = tx + tw;
            u1 = tx;
            u2 = tx;
            u3 = tx + tw;
        }

        // Apply rotations
        let mut t = |pos: Vec3| -> Vec3 {
            let mut pt = pos - part.pivot;
            pt = rotate_z(pt, part.rot.z);
            pt = rotate_x(pt, part.rot.x);
            pt = rotate_y(pt, part.rot.y);
            pt + part.pivot
        };

        let tp0 = t(p0);
        let tp1 = t(p1);
        let tp2 = t(p2);
        let tp3 = t(p3);

        // Correct winding for CCW in screen space (Y-down): TL, BL, BR and TL, BR, TR
        // This ensures triangles are NOT culled when facing the camera.
        let vec1 = tp3 - tp0;
        let vec2 = tp2 - tp0;
        let normal = vec1.cross(vec2).normalize();

        tris.push(Triangle {
            v: [(tp0, (u0, v0)), (tp3, (u3, v3)), (tp2, (u2, v2))],
            normal,
        });
        tris.push(Triangle {
            v: [(tp0, (u0, v0)), (tp2, (u2, v2)), (tp1, (u1, v1))],
            normal,
        });
    };

    // Minecraft skin UV mapping - standard cube faces (Top-Left, Top-Right, Bottom-Right, Bottom-Left)
    // Front face (touching player back for cape, chest for others)
    let front_u = if part.limb == Limb::Cape { u + d + w + d } else { u + d };
    add_face(p011, p111, p101, p001, front_u, v + d, w, h);
    // Back face (visible part for cape, back for others)
    let back_u = if part.limb == Limb::Cape { u + d } else { u + d + w + d };
    add_face(p110, p010, p000, p100, back_u, v + d, w, h);
    // Right face
    let right_u = if part.limb == Limb::Cape { u + d + w } else { u };
    add_face(p111, p110, p100, p101, right_u, v + d, d, h);
    // Left face
    let left_u = if part.limb == Limb::Cape { u } else { u + d + w };
    add_face(p010, p011, p001, p000, left_u, v + d, d, h);
    // Top face
    add_face(p010, p110, p111, p011, u + d, v, w, d);
    // Bottom face
    add_face(p001, p101, p100, p000, u + d + w, v, w, d);

    tris
}

fn edge_function(a: Vec3, b: Vec3, c: Vec3) -> f32 {
    (c.x - a.x) * (b.y - a.y) - (c.y - a.y) * (b.x - a.x)
}

pub struct SkinRenderer {
    pub image_bytes: Option<Arc<[u8]>>,
    parsed_image: Option<RgbaImage>,
    start_time: Instant,
    yaw: f32,
    pitch: f32,
    is_dragging: bool,
    last_mouse: Option<Point<Pixels>>,
    pub slim: bool,
    pub is_static: bool,
    pub nameplate: Option<SharedString>,
    pub cape_bytes: Option<Arc<[u8]>>,
    parsed_cape: Option<RgbaImage>,
    _window_event_subscription: Option<Subscription>,
}

impl SkinRenderer {
    pub fn new(image_bytes: Option<Arc<[u8]>>, slim: bool) -> Self {
        let mut parsed_image = None;
        if let Some(bytes) = &image_bytes {
            if let Ok(img) = image::load_from_memory(bytes) {
                parsed_image = Some(img.to_rgba8());
            }
        }
        Self {
            image_bytes,
            parsed_image,
            start_time: Instant::now(),
            yaw: 0.5,
            pitch: 0.1,
            is_dragging: false,
            last_mouse: None,
            slim,
            is_static: false,
            nameplate: None,
            cape_bytes: None,
            parsed_cape: None,
            _window_event_subscription: None,
        }
    }

    pub fn update_image(&mut self, image_bytes: Option<Arc<[u8]>>, slim: bool) {
        if let Some(new_bytes) = &image_bytes {
            let should_update = if let Some(old_bytes) = &self.image_bytes {
                !Arc::ptr_eq(old_bytes, new_bytes)
            } else {
                true
            };
            if should_update {
                self.image_bytes = Some(new_bytes.clone());
                if let Ok(img) = image::load_from_memory(new_bytes) {
                    self.parsed_image = Some(img.to_rgba8());
                }
            }
            self.slim = slim;
        } else {
            self.image_bytes = None;
            self.parsed_image = None;
        }
    }

    pub fn update_cape(&mut self, cape_bytes: Option<Arc<[u8]>>) {
        if let Some(new_bytes) = &cape_bytes {
            let should_update = if let Some(old_bytes) = &self.cape_bytes {
                !Arc::ptr_eq(old_bytes, new_bytes)
            } else {
                true
            };
            if should_update {
                if let Ok(img) = image::load_from_memory(new_bytes) {
                    self.parsed_cape = Some(img.to_rgba8());
                }
            }
        } else {
            self.parsed_cape = None;
        }
        self.cape_bytes = cape_bytes;
    }

    pub fn render_to_buffer(&self, width: u32, height: u32) -> Option<Arc<RenderImage>> {
        self.render_to_buffer_with_params(width, height, self.yaw, self.pitch, self.is_static)
    }

    pub fn render_to_buffer_with_params(&self, width: u32, height: u32, yaw: f32, pitch: f32, is_static: bool) -> Option<Arc<RenderImage>> {
        let tex = self.parsed_image.as_ref()?;
        let is_64x32 = tex.height() == 32;

        let mut zbuf = vec![std::f32::MIN; (width * height) as usize];
        let mut colorbuf = vec![0u8; (width * height * 4) as usize];

        let time = if is_static { 2.0 } else { self.start_time.elapsed().as_secs_f32() };
        let mut parts = build_parts(self.slim);

        // Animations - slightly more complex to mimic Modrinth
        let breathe = if is_static { 0.0 } else { (time * 1.8).sin() * 0.4 };
        let swing_base = if is_static { 0.0 } else { (time * 1.5).sin() };
        let arm_swing = swing_base * 0.3;
        let leg_swing = swing_base * 0.4;

        // Modrinth-style discrete sub-animations
        let sub_cycle = (time / 8.0).floor() as u32;
        let sub_inner = time % 8.0;
        let mut head_sub_tilt = 0.0;
        let mut head_sub_yaw = 0.0;
        let mut arm_sub_lift = 0.0;

        if sub_inner < 1.5 {
            let t = sub_inner / 1.5;
            let pulse = (t * std::f32::consts::PI).sin();
            match sub_cycle % 3 {
                0 => head_sub_tilt = pulse * 0.1,
                1 => head_sub_yaw = pulse * 0.2,
                2 => arm_sub_lift = pulse * 0.15,
                _ => {}
            }
        }

        for p in parts.iter_mut() {
            // Breathing
            if p.pos.y >= 12.0 {
                p.pos.y += breathe;
                p.pivot.y += breathe;
            }

            // Limb swinging
            match p.limb {
                Limb::RightArm => p.rot.x = -arm_swing + arm_sub_lift,
                Limb::LeftArm => p.rot.x = arm_swing,
                Limb::RightLeg => p.rot.x = leg_swing,
                Limb::LeftLeg => p.rot.x = -leg_swing,
                Limb::Cape => p.rot.x = 0.1 + (time * 1.5).cos().abs() * 0.4,
                _ => {}
            }
            
            // Random head look
            if p.limb == Limb::Head {
                p.rot.y = (time * 0.4).sin() * 0.15 + head_sub_yaw;
                p.rot.x = (time * 0.3).cos() * 0.05 + head_sub_tilt;
            }
        }

        let is_card = width <= 200 && height <= 200;
        let scale = if is_card {
            height as f32 / 20.0
        } else {
            height as f32 / 38.0
        };
        let offset_x = width as f32 / 2.0;
        let offset_y = if is_card {
            height as f32 / 2.0 + 26.0 * scale
        } else {
            height as f32 / 2.0 + 18.0 * scale
        };

        let global_pitch = pitch;
        let global_yaw = yaw;
        let light_dir = Vec3 { x: -3.0, y: 4.0, z: 2.0 }.normalize(); // Light from front-top-left

        for part in parts {
            let tex_to_use = if part.limb == Limb::Cape {
                if let Some(cape) = &self.parsed_cape {
                    cape
                } else {
                    continue;
                }
            } else {
                tex
            };

            let tris = generate_triangles(&part, is_64x32);
            for t in tris {
                // Project normal
                let mut norm = t.normal;
                norm = rotate_x(norm, global_pitch);
                norm = rotate_y(norm, global_yaw);
                let light_intensity = (norm.dot(light_dir).max(0.0) * 1.0 + 0.4).min(1.5);

                let mut v_proj = [(Vec3 { x: 0., y: 0., z: 0. }, (0., 0.)); 3];
                for i in 0..3 {
                    // Global rot
                    let mut pos = t.v[i].0;
                    pos.y -= 16.0;
                    pos = rotate_x(pos, global_pitch);
                    pos = rotate_y(pos, global_yaw);
                    pos.y += 16.0;

                    let screen_x = pos.x * scale + offset_x;
                    let screen_y = offset_y - pos.y * scale;
                    v_proj[i] = (
                        Vec3 {
                            x: screen_x,
                            y: screen_y,
                            z: pos.z,
                        },
                        t.v[i].1,
                    );
                }

                // backface culling
                let area = edge_function(v_proj[0].0, v_proj[1].0, v_proj[2].0);
                if area <= 0.0 {
                    continue;
                }

                let inv_area = 1.0 / area;
                let min_x = (v_proj[0].0.x.min(v_proj[1].0.x).min(v_proj[2].0.x).floor() as i32).max(0);
                let max_x = (v_proj[0].0.x.max(v_proj[1].0.x).max(v_proj[2].0.x).ceil() as i32).min((width - 1) as i32);
                let min_y = (v_proj[0].0.y.min(v_proj[1].0.y).min(v_proj[2].0.y).floor() as i32).max(0);
                let max_y =
                    (v_proj[0].0.y.max(v_proj[1].0.y).max(v_proj[2].0.y).ceil() as i32).min((height - 1) as i32);

                for y in min_y..=max_y {
                    for x in min_x..=max_x {
                        let px = x as f32 + 0.5;
                        let py = y as f32 + 0.5;
                        let p = Vec3 { x: px, y: py, z: 0.0 };
                        let w0 = edge_function(v_proj[1].0, v_proj[2].0, p) * inv_area;
                        let w1 = edge_function(v_proj[2].0, v_proj[0].0, p) * inv_area;
                        let w2 = edge_function(v_proj[0].0, v_proj[1].0, p) * inv_area;

                        if w0 >= -0.001 && w1 >= -0.001 && w2 >= -0.001 {
                            let z = w0 * v_proj[0].0.z + w1 * v_proj[1].0.z + w2 * v_proj[2].0.z;
                            let idx = (y as usize) * (width as usize) + (x as usize);

                            if z > zbuf[idx] {
                                let u = w0 * v_proj[0].1 .0 + w1 * v_proj[1].1 .0 + w2 * v_proj[2].1 .0;
                                let v = w0 * v_proj[0].1 .1 + w1 * v_proj[1].1 .1 + w2 * v_proj[2].1 .1;

                                let tx = (u * (tex_to_use.width() as f32 / 64.0)).clamp(0.0, (tex_to_use.width() - 1) as f32) as u32;
                                let ty = if part.limb == Limb::Cape {
                                    (v * (tex_to_use.height() as f32 / 32.0)).clamp(0.0, (tex_to_use.height() - 1) as f32) as u32
                                } else {
                                    (v * (tex_to_use.height() as f32 / if is_64x32 { 32.0 } else { 64.0 }))
                                        .clamp(0.0, (tex_to_use.height() - 1) as f32)
                                        as u32
                                };

                                let pixel = tex_to_use.get_pixel(tx, ty);
                                if pixel[3] > 128 {
                                    zbuf[idx] = z;
                                    let cidx = idx * 4;
                                    colorbuf[cidx] = (pixel[2] as f32 * light_intensity).min(255.0) as u8;
                                    colorbuf[cidx + 1] = (pixel[1] as f32 * light_intensity).min(255.0) as u8;
                                    colorbuf[cidx + 2] = (pixel[0] as f32 * light_intensity).min(255.0) as u8;
                                    colorbuf[cidx + 3] = pixel[3];
                                }
                            }
                        }
                    }
                }
            }
        }

        if let Some(img) = RgbaImage::from_raw(width, height, colorbuf) {
            let frame = Frame::new(img);
            Some(Arc::new(RenderImage::new(vec![frame])))
        } else {
            None
        }
    }
}

impl Render for SkinRenderer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity().clone();

        if self._window_event_subscription.is_none() {
            // Global mouse up reset - skip for now if on_window_event is missing
        }

        div()
            .size_full()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, e: &MouseDownEvent, _, _| {
                    this.is_dragging = true;
                    this.last_mouse = Some(e.position);
                }),
            )
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| {
                    this.is_dragging = false;
                    this.last_mouse = None;
                }),
            )
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, _| {
                if this.is_dragging {
                    if let Some(last) = this.last_mouse {
                        let e_x: f32 = e.position.x.into();
                        let l_x: f32 = last.x.into();
                        let dx = e_x - l_x;
                        this.yaw -= dx * 0.01;
                    }
                    this.last_mouse = Some(e.position);
                }
            }))
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, cx| {
                        window.request_animation_frame();
                        let w_f32: f32 = bounds.size.width.into();
                        let h_f32: f32 = bounds.size.height.into();
                        let w = w_f32 as u32;
                        let h = h_f32 as u32;
                        if w > 0 && h > 0 {
                            if let Some(render_img) = entity.read(cx).render_to_buffer(w, h) {
                                let _ = window.paint_image(bounds, gpui::Corners::default(), render_img, 0, false);
                            }
                        }
                    },
                )
                .size_full(),
            )
            .when_some(self.nameplate.clone(), |this, name| {
                this.child(
                    div()
                        .absolute()
                        .top_1()
                        .w_full()
                        .h_flex()
                        .justify_center()
                        .child(
                            div()
                                .px_3()
                                .py_1()
                                .bg(gpui::rgba(0x000000a0))
                                .rounded_md()
                                .child(name)
                                .text_lg()
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_color(gpui::white())
                        )
                )
            })
    }
}
