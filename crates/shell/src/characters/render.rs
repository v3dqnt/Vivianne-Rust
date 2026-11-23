//! A small software renderer for the Kenney mini characters (skinned glTF models): it poses a model at a point of one
//! of its animations, skins the mesh, and draws it with a depth buffer and flat shading to a transparent picture. No
//! graphics API is involved, so it works the same everywhere and the pictures can be cached as plain PNG files.

use std::f32::consts::PI;

type V3 = [f32; 3];
/// Column-major 4x4.
type M4 = [f32; 16];

const IDENTITY: M4 = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

fn mul(a: &M4, b: &M4) -> M4 {
    let mut out = [0.0; 16];
    for col in 0..4 {
        for row in 0..4 {
            out[col * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[col * 4 + k]).sum();
        }
    }
    out
}

fn from_trs(t: V3, q: [f32; 4], s: V3) -> M4 {
    let [x, y, z, w] = q;
    let (xx, yy, zz) = (x * x, y * y, z * z);
    let (xy, xz, yz) = (x * y, x * z, y * z);
    let (wx, wy, wz) = (w * x, w * y, w * z);
    [
        (1.0 - 2.0 * (yy + zz)) * s[0],
        2.0 * (xy + wz) * s[0],
        2.0 * (xz - wy) * s[0],
        0.0,
        2.0 * (xy - wz) * s[1],
        (1.0 - 2.0 * (xx + zz)) * s[1],
        2.0 * (yz + wx) * s[1],
        0.0,
        2.0 * (xz + wy) * s[2],
        2.0 * (yz - wx) * s[2],
        (1.0 - 2.0 * (xx + yy)) * s[2],
        0.0,
        t[0],
        t[1],
        t[2],
        1.0,
    ]
}

fn point(m: &M4, p: V3) -> V3 {
    [
        m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12],
        m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13],
        m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14],
    ]
}

fn slerp(a: [f32; 4], mut b: [f32; 4], t: f32) -> [f32; 4] {
    let mut dot: f32 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
    if dot < 0.0 {
        dot = -dot;
        for v in &mut b {
            *v = -*v;
        }
    }
    let (ka, kb) = if dot > 0.9995 {
        (1.0 - t, t)
    } else {
        let theta = dot.acos();
        let sin = theta.sin();
        (((1.0 - t) * theta).sin() / sin, (t * theta).sin() / sin)
    };
    let mut out = [0.0; 4];
    for i in 0..4 {
        out[i] = a[i] * ka + b[i] * kb;
    }
    let len = out.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-9);
    out.map(|v| v / len)
}

fn lerp3(a: V3, b: V3, t: f32) -> V3 {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// What one animation channel moves.
enum Track {
    Translation(Vec<V3>),
    Rotation(Vec<[f32; 4]>),
    Scale(Vec<V3>),
}

struct Channel {
    node: usize,
    times: Vec<f32>,
    track: Track,
    step: bool,
}

struct Animation {
    name: String,
    duration: f32,
    channels: Vec<Channel>,
}

struct Part {
    positions: Vec<V3>,
    uvs: Vec<[f32; 2]>,
    joints: Vec<[u16; 4]>,
    weights: Vec<[f32; 4]>,
    indices: Vec<u32>,
    /// Joint nodes and their inverse bind matrices.
    skin: Option<(Vec<usize>, Vec<M4>)>,
    /// For a part that is not skinned: the node it hangs from.
    node: usize,
}

pub struct Model {
    parent: Vec<Option<usize>>,
    rest: Vec<(V3, [f32; 4], V3)>,
    parts: Vec<Part>,
    animations: Vec<Animation>,
    texture: Texture,
}

struct Texture {
    w: usize,
    h: usize,
    px: Vec<[u8; 4]>,
}

impl Texture {
    fn sample(&self, uv: [f32; 2]) -> [u8; 4] {
        let x = ((uv[0] * self.w as f32) as isize).clamp(0, self.w as isize - 1) as usize;
        let y = ((uv[1] * self.h as f32) as isize).clamp(0, self.h as isize - 1) as usize;
        self.px[y * self.w + x]
    }
}

impl Model {
    /// Reads a binary glTF and its colour map picture.
    pub fn load(glb: &[u8], colormap_png: &[u8]) -> Result<Self, String> {
        let g = gltf::Gltf::from_slice(glb).map_err(|e| e.to_string())?;
        let blob = g.blob.clone().ok_or("the model has no data")?;
        let doc = &g.document;
        let data = |b: gltf::Buffer| -> Option<&[u8]> {
            let _ = b;
            Some(&blob[..])
        };

        let count = doc.nodes().count();
        let mut parent = vec![None; count];
        let mut rest = Vec::with_capacity(count);
        for node in doc.nodes() {
            for child in node.children() {
                parent[child.index()] = Some(node.index());
            }
            let (t, r, s) = node.transform().decomposed();
            rest.push((t, r, s));
        }

        let mut parts = Vec::new();
        for node in doc.nodes() {
            let Some(mesh) = node.mesh() else { continue };
            let skin = node.skin().map(|skin| {
                let joints: Vec<usize> = skin.joints().map(|j| j.index()).collect();
                let ibm: Vec<M4> = skin
                    .reader(data)
                    .read_inverse_bind_matrices()
                    .map(|it| {
                        it.map(|m| {
                            let mut flat = [0.0; 16];
                            for (c, col) in m.iter().enumerate() {
                                flat[c * 4..c * 4 + 4].copy_from_slice(col);
                            }
                            flat
                        })
                        .collect()
                    })
                    .unwrap_or_else(|| vec![IDENTITY; joints.len()]);
                (joints, ibm)
            });
            for prim in mesh.primitives() {
                let reader = prim.reader(data);
                let positions: Vec<V3> = reader
                    .read_positions()
                    .ok_or("a mesh has no positions")?
                    .collect();
                let uvs = reader
                    .read_tex_coords(0)
                    .map(|t| t.into_f32().collect())
                    .unwrap_or_else(|| vec![[0.0, 0.0]; positions.len()]);
                let joints = reader
                    .read_joints(0)
                    .map(|j| j.into_u16().collect())
                    .unwrap_or_default();
                let weights = reader
                    .read_weights(0)
                    .map(|w| w.into_f32().collect())
                    .unwrap_or_default();
                let indices = reader.read_indices().map_or_else(
                    || (0..positions.len() as u32).collect(),
                    |i| i.into_u32().collect(),
                );
                parts.push(Part {
                    positions,
                    uvs,
                    joints,
                    weights,
                    indices,
                    skin: skin.clone(),
                    node: node.index(),
                });
            }
        }

        let mut animations = Vec::new();
        for anim in doc.animations() {
            let mut channels = Vec::new();
            let mut duration = 0.0_f32;
            for ch in anim.channels() {
                let reader = ch.reader(data);
                let times: Vec<f32> = reader.read_inputs().ok_or("no times")?.collect();
                duration = duration.max(times.last().copied().unwrap_or(0.0));
                let step = ch.sampler().interpolation() == gltf::animation::Interpolation::Step;
                let cubic =
                    ch.sampler().interpolation() == gltf::animation::Interpolation::CubicSpline;
                let pick = |n: usize| if cubic { n * 3 + 1 } else { n };
                let track = match reader.read_outputs().ok_or("no values")? {
                    gltf::animation::util::ReadOutputs::Translations(it) => {
                        let all: Vec<V3> = it.collect();
                        Track::Translation((0..times.len()).map(|i| all[pick(i)]).collect())
                    }
                    gltf::animation::util::ReadOutputs::Rotations(r) => {
                        let all: Vec<[f32; 4]> = r.into_f32().collect();
                        Track::Rotation((0..times.len()).map(|i| all[pick(i)]).collect())
                    }
                    gltf::animation::util::ReadOutputs::Scales(it) => {
                        let all: Vec<V3> = it.collect();
                        Track::Scale((0..times.len()).map(|i| all[pick(i)]).collect())
                    }
                    gltf::animation::util::ReadOutputs::MorphTargetWeights(_) => continue,
                };
                channels.push(Channel {
                    node: ch.target().node().index(),
                    times,
                    track,
                    step,
                });
            }
            animations.push(Animation {
                name: anim.name().unwrap_or_default().to_owned(),
                duration,
                channels,
            });
        }

        let img = image::load_from_memory(colormap_png)
            .map_err(|e| e.to_string())?
            .to_rgba8();
        let texture = Texture {
            w: img.width() as usize,
            h: img.height() as usize,
            px: img.pixels().map(|p| p.0).collect(),
        };
        Ok(Self {
            parent,
            rest,
            parts,
            animations,
            texture,
        })
    }

    /// The animation called `name`, else the first.
    fn animation(&self, name: &str) -> Option<&Animation> {
        self.animations
            .iter()
            .find(|a| a.name == name)
            .or_else(|| self.animations.first())
    }

    /// Names of the animations the model has.
    #[must_use]
    pub fn animation_names(&self) -> Vec<String> {
        self.animations.iter().map(|a| a.name.clone()).collect()
    }

    /// How long the animation runs, in seconds.
    #[must_use]
    pub fn duration(&self, name: &str) -> f32 {
        self.animation(name).map_or(0.0, |a| a.duration)
    }

    fn world_matrices(&self, anim: Option<&Animation>, time: f32) -> Vec<M4> {
        let mut local = self.rest.clone();
        if let Some(anim) = anim {
            for ch in &anim.channels {
                let (i, j, f) = segment(&ch.times, time, ch.step);
                match &ch.track {
                    Track::Translation(v) => local[ch.node].0 = lerp3(v[i], v[j], f),
                    Track::Rotation(v) => local[ch.node].1 = slerp(v[i], v[j], f),
                    Track::Scale(v) => local[ch.node].2 = lerp3(v[i], v[j], f),
                }
            }
        }
        let mut world: Vec<Option<M4>> = vec![None; local.len()];
        fn resolve(
            i: usize,
            parent: &[Option<usize>],
            local: &[(V3, [f32; 4], V3)],
            world: &mut Vec<Option<M4>>,
        ) -> M4 {
            if let Some(m) = world[i] {
                return m;
            }
            let own = from_trs(local[i].0, local[i].1, local[i].2);
            let m = match parent[i] {
                Some(p) => mul(&resolve(p, parent, local, world), &own),
                None => own,
            };
            world[i] = Some(m);
            m
        }
        (0..local.len())
            .map(|i| resolve(i, &self.parent, &local, &mut world))
            .collect()
    }

    /// Every triangle of the model at `time` seconds into the animation: positions and texture coordinates.
    fn posed(&self, anim: Option<&Animation>, time: f32) -> Vec<([V3; 3], [[f32; 2]; 3])> {
        let world = self.world_matrices(anim, time);
        let mut tris = Vec::new();
        for part in &self.parts {
            let joint_matrices: Option<Vec<M4>> = part.skin.as_ref().map(|(joints, ibm)| {
                joints
                    .iter()
                    .zip(ibm)
                    .map(|(j, ibm)| mul(&world[*j], ibm))
                    .collect()
            });
            let skinned: Vec<V3> = part
                .positions
                .iter()
                .enumerate()
                .map(|(i, p)| match (&joint_matrices, part.joints.get(i)) {
                    (Some(mats), Some(js)) => {
                        let w = part.weights.get(i).copied().unwrap_or([1.0, 0.0, 0.0, 0.0]);
                        let mut out = [0.0; 3];
                        for k in 0..4 {
                            if w[k] > 0.0 {
                                let q = point(&mats[js[k] as usize], *p);
                                for c in 0..3 {
                                    out[c] += q[c] * w[k];
                                }
                            }
                        }
                        out
                    }
                    _ => point(&world[part.node], *p),
                })
                .collect();
            for tri in part.indices.chunks_exact(3) {
                let ix = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
                tris.push((
                    [skinned[ix[0]], skinned[ix[1]], skinned[ix[2]]],
                    [part.uvs[ix[0]], part.uvs[ix[1]], part.uvs[ix[2]]],
                ));
            }
        }
        tris
    }

    /// Draws `frames` pictures spread over one loop of the animation, `size` pixels square, all framed the same.
    #[must_use]
    pub fn render_loop(&self, animation: &str, frames: usize, size: usize) -> Vec<Vec<u8>> {
        const SS: usize = 3;
        const YAW: f32 = -0.55;
        const PITCH: f32 = 0.12;
        let anim = self.animation(animation);
        let duration = anim.map_or(0.0, |a| a.duration).max(0.001);
        let (sy, cy) = YAW.sin_cos();
        let (sp, cp) = PITCH.sin_cos();
        // World to view: turn about the up axis, then tip toward the camera.
        let view = |p: V3| -> V3 {
            let x = p[0] * cy + p[2] * sy;
            let z = -p[0] * sy + p[2] * cy;
            let y = p[1] * cp - z * sp;
            let z = p[1] * sp + z * cp;
            [x, y, z]
        };
        let posed: Vec<Vec<([V3; 3], [[f32; 2]; 3])>> = (0..frames)
            .map(|f| {
                let time = duration * f as f32 / frames as f32;
                self.posed(anim, time)
                    .into_iter()
                    .map(|(p, uv)| ([view(p[0]), view(p[1]), view(p[2])], uv))
                    .collect()
            })
            .collect();
        let (mut min, mut max) = ([f32::MAX; 2], [f32::MIN; 2]);
        for tris in &posed {
            for (p, _) in tris {
                for v in p {
                    for c in 0..2 {
                        min[c] = min[c].min(v[c]);
                        max[c] = max[c].max(v[c]);
                    }
                }
            }
        }
        let span = (max[0] - min[0]).max(max[1] - min[1]).max(1e-4) * 1.12;
        let scale = (size * SS) as f32 / span;
        let centre = [(min[0] + max[0]) / 2.0, (min[1] + max[1]) / 2.0];
        let big = size * SS;
        let light = {
            let l: V3 = [0.35, 0.65, 0.7];
            let n = (l[0] * l[0] + l[1] * l[1] + l[2] * l[2]).sqrt();
            [l[0] / n, l[1] / n, l[2] / n]
        };
        let _ = PI;

        posed
            .iter()
            .map(|tris| {
                let mut color = vec![[0u8; 4]; big * big];
                let mut depth = vec![f32::MIN; big * big];
                for (p, uv) in tris {
                    let n = normal(p);
                    if n[2] <= 0.0 {
                        continue;
                    }
                    let shade = 0.5 + 0.5 * (n[0] * light[0] + n[1] * light[1] + n[2] * light[2]).max(0.0);
                    let s: Vec<[f32; 3]> = p
                        .iter()
                        .map(|v| {
                            [
                                (v[0] - centre[0]) * scale + big as f32 / 2.0,
                                big as f32 / 2.0 - (v[1] - centre[1]) * scale,
                                v[2],
                            ]
                        })
                        .collect();
                    let min_x = s.iter().map(|v| v[0]).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
                    let max_x = (s.iter().map(|v| v[0]).fold(f32::MIN, f32::max).ceil() as usize).min(big - 1);
                    let min_y = s.iter().map(|v| v[1]).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
                    let max_y = (s.iter().map(|v| v[1]).fold(f32::MIN, f32::max).ceil() as usize).min(big - 1);
                    let area = edge(&s[0], &s[1], s[2][0], s[2][1]);
                    if area.abs() < 1e-6 {
                        continue;
                    }
                    for y in min_y..=max_y {
                        for x in min_x..=max_x {
                            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                            let w0 = edge(&s[1], &s[2], px, py) / area;
                            let w1 = edge(&s[2], &s[0], px, py) / area;
                            let w2 = edge(&s[0], &s[1], px, py) / area;
                            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                                continue;
                            }
                            let z = w0 * s[0][2] + w1 * s[1][2] + w2 * s[2][2];
                            let at = y * big + x;
                            if z <= depth[at] {
                                continue;
                            }
                            let u = [
                                w0 * uv[0][0] + w1 * uv[1][0] + w2 * uv[2][0],
                                w0 * uv[0][1] + w1 * uv[1][1] + w2 * uv[2][1],
                            ];
                            let t = self.texture.sample(u);
                            depth[at] = z;
                            color[at] = [
                                (f32::from(t[0]) * shade) as u8,
                                (f32::from(t[1]) * shade) as u8,
                                (f32::from(t[2]) * shade) as u8,
                                255,
                            ];
                        }
                    }
                }
                downsample(&color, big, SS)
            })
            .collect()
    }
}

/// The two keys around `time` and how far between them (a held key for a step track).
fn segment(times: &[f32], time: f32, step: bool) -> (usize, usize, f32) {
    let last = times.len() - 1;
    if time <= times[0] {
        return (0, 0, 0.0);
    }
    if time >= times[last] {
        return (last, last, 0.0);
    }
    let j = times.partition_point(|t| *t <= time);
    let i = j - 1;
    let f = if step {
        0.0
    } else {
        (time - times[i]) / (times[j] - times[i]).max(1e-9)
    };
    (i, j, f)
}

fn edge(a: &[f32; 3], b: &[f32; 3], px: f32, py: f32) -> f32 {
    (b[0] - a[0]) * (py - a[1]) - (b[1] - a[1]) * (px - a[0])
}

fn normal(p: &[V3; 3]) -> V3 {
    let a = [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]];
    let b = [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]];
    let n = [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ];
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-12);
    [n[0] / len, n[1] / len, n[2] / len]
}

/// Averages `ss` x `ss` blocks into one pixel, weighting colour by coverage so the edge stays clean.
fn downsample(color: &[[u8; 4]], big: usize, ss: usize) -> Vec<u8> {
    let size = big / ss;
    let mut out = Vec::with_capacity(size * size * 4);
    for y in 0..size {
        for x in 0..size {
            let (mut r, mut g, mut b, mut a) = (0u32, 0u32, 0u32, 0u32);
            for dy in 0..ss {
                for dx in 0..ss {
                    let p = color[(y * ss + dy) * big + x * ss + dx];
                    if p[3] > 0 {
                        r += u32::from(p[0]);
                        g += u32::from(p[1]);
                        b += u32::from(p[2]);
                        a += 1;
                    }
                }
            }
            let total = (ss * ss) as u32;
            if a == 0 {
                out.extend_from_slice(&[0, 0, 0, 0]);
            } else {
                out.extend_from_slice(&[
                    (r / a) as u8,
                    (g / a) as u8,
                    (b / a) as u8,
                    (a * 255 / total) as u8,
                ]);
            }
        }
    }
    out
}
