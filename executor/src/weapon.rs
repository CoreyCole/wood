use crate::{
    actor::Actor, actor::ActorContainer, message::Message, projectile::ProjectileKind, GameTime,
};
use fyrox::{
    core::{
        algebra::{Matrix3, Point3, Vector3},
        color::Color,
        math::{ray::Ray, Matrix4Ext, Vector3Ext},
        pool::{Handle, Pool},
        visitor::{Visit, VisitResult, Visitor},
    },
    engine::resource_manager::ResourceManager,
    scene::{
        base::BaseBuilder,
        collider::InteractionGroups,
        graph::{physics::RayCastOptions, Graph},
        light::{point::PointLightBuilder, BaseLightBuilder},
        node::Node,
        Scene,
    },
    utils::log::{Log, MessageKind},
};
use std::{
    ops::{Index, IndexMut},
    path::{Path, PathBuf},
    sync::mpsc::Sender,
};

#[derive(Copy, Clone, PartialEq, Eq, Debug, Visit)]
pub enum WeaponKind {
    M4,
    Ak47,
    PlasmaRifle,
    RocketLauncher,
    BattleAxe,
}

#[derive(Visit)]
pub struct Weapon {
    kind: WeaponKind,
    model: Handle<Node>,
    laser_dot: Handle<Node>,
    shot_point: Handle<Node>,
    offset: Vector3<f32>,
    dest_offset: Vector3<f32>,
    last_shot_time: f64,
    shot_position: Vector3<f32>,
    owner: Handle<Actor>,
    ammo: u32,
    #[visit(skip)]
    pub sender: Option<Sender<Message>>,
}

pub struct WeaponDefinition {
    pub model: &'static str,
    pub shot_sound: &'static str,
    pub ammo: u32,
    pub projectile: ProjectileKind,
    pub shoot_interval: f64,
}

impl Default for Weapon {
    fn default() -> Self {
        Self {
            kind: WeaponKind::M4,
            laser_dot: Handle::NONE,
            model: Handle::NONE,
            offset: Vector3::default(),
            shot_point: Handle::NONE,
            dest_offset: Vector3::default(),
            last_shot_time: 0.0,
            shot_position: Vector3::default(),
            owner: Handle::NONE,
            ammo: 250,
            sender: None,
        }
    }
}

impl Weapon {
    pub fn get_definition(kind: WeaponKind) -> &'static WeaponDefinition {
        match kind {
            WeaponKind::BattleAxe => {
                static DEFINITION: WeaponDefinition = WeaponDefinition {
                    model: "data/models/ak47.FBX",
                    shot_sound: "data/sounds/axe-swing-1.ogg",
                    ammo: 200,
                    projectile: ProjectileKind::Bullet,
                    shoot_interval: 0.15,
                };
                &DEFINITION
            }
            WeaponKind::M4 => {
                static DEFINITION: WeaponDefinition = WeaponDefinition {
                    model: "data/models/m4.FBX",
                    shot_sound: "data/sounds/m4_shot.ogg",
                    ammo: 200,
                    projectile: ProjectileKind::Bullet,
                    shoot_interval: 0.15,
                };
                &DEFINITION
            }
            WeaponKind::Ak47 => {
                static DEFINITION: WeaponDefinition = WeaponDefinition {
                    model: "data/models/ak47.FBX",
                    shot_sound: "data/sounds/ak47.ogg",
                    ammo: 200,
                    projectile: ProjectileKind::Bullet,
                    shoot_interval: 0.15,
                };
                &DEFINITION
            }
            WeaponKind::PlasmaRifle => {
                static DEFINITION: WeaponDefinition = WeaponDefinition {
                    model: "data/models/plasma_rifle.FBX",
                    shot_sound: "data/sounds/plasma_shot.ogg",
                    ammo: 100,
                    projectile: ProjectileKind::Plasma,
                    shoot_interval: 0.25,
                };
                &DEFINITION
            }
            WeaponKind::RocketLauncher => {
                static DEFINITION: WeaponDefinition = WeaponDefinition {
                    model: "data/models/Rpg7.FBX",
                    shot_sound: "data/sounds/grenade_launcher_fire.ogg",
                    ammo: 100,
                    projectile: ProjectileKind::Rocket,
                    shoot_interval: 1.5,
                };
                &DEFINITION
            }
        }
    }

    pub async fn new(
        kind: WeaponKind,
        resource_manager: ResourceManager,
        scene: &mut Scene,
        sender: Sender<Message>,
    ) -> Weapon {
        let definition = Self::get_definition(kind);

        let model = resource_manager
            .request_model(Path::new(definition.model))
            .await
            .unwrap()
            .instantiate_geometry(scene);

        let laser_dot = PointLightBuilder::new(
            BaseLightBuilder::new(BaseBuilder::new())
                .with_color(Color::opaque(255, 0, 0))
                .with_scatter_enabled(false)
                .cast_shadows(false),
        )
        .with_radius(0.5)
        .build(&mut scene.graph);

        let shot_point = scene.graph.find_by_name(model, "Weapon:ShotPoint");

        if shot_point.is_none() {
            Log::writeln(MessageKind::Warning, "Shot point not found!".to_owned());
        }

        Weapon {
            kind,
            laser_dot,
            model,
            shot_point,
            ammo: definition.ammo,
            sender: Some(sender),
            ..Default::default()
        }
    }

    pub fn set_visibility(&self, visibility: bool, graph: &mut Graph) {
        graph[self.model].set_visibility(visibility);
        graph[self.laser_dot].set_visibility(visibility);
    }

    pub fn get_model(&self) -> Handle<Node> {
        self.model
    }

    pub fn update(&mut self, scene: &mut Scene, actors: &ActorContainer) {
        self.offset.follow(&self.dest_offset, 0.2);

        self.update_laser_sight(&mut scene.graph, actors);

        let node = &mut scene.graph[self.model];
        node.local_transform_mut().set_position(self.offset);
        self.shot_position = node.global_position();
    }

    pub fn get_shot_position(&self, graph: &Graph) -> Vector3<f32> {
        if self.shot_point.is_some() {
            graph[self.shot_point].global_position()
        } else {
            // Fallback
            graph[self.model].global_position()
        }
    }

    pub fn get_shot_direction(&self, graph: &Graph) -> Vector3<f32> {
        graph[self.model].look_vector()
    }

    pub fn get_kind(&self) -> WeaponKind {
        self.kind
    }

    pub fn world_basis(&self, graph: &Graph) -> Matrix3<f32> {
        graph[self.model].global_transform().basis()
    }

    pub fn add_ammo(&mut self, amount: u32) {
        self.ammo += amount;
    }

    fn update_laser_sight(&self, graph: &mut Graph, actors: &ActorContainer) {
        let mut laser_dot_position = Vector3::default();
        let model = &graph[self.model];
        let begin = model.global_position();
        let end = begin + model.look_vector().scale(100.0);
        let ray = Ray::from_two_points(begin, end);
        let mut query_buffer = Vec::default();
        graph.physics.cast_ray(
            RayCastOptions {
                ray_origin: Point3::from(ray.origin),
                ray_direction: ray.dir,
                max_len: f32::MAX,
                groups: InteractionGroups::default(),
                sort_results: true,
            },
            &mut query_buffer,
        );
        'hit_loop: for hit in query_buffer.iter() {
            // Filter hit with owner capsule
            for (handle, actor) in actors.pair_iter() {
                if self.owner == handle && actor.collider == hit.collider {
                    continue 'hit_loop;
                }
            }

            let offset = hit
                .normal
                .try_normalize(std::f32::EPSILON)
                .unwrap_or_default()
                .scale(0.2);
            laser_dot_position = hit.position.coords + offset;
            break 'hit_loop;
        }

        graph[self.laser_dot]
            .local_transform_mut()
            .set_position(laser_dot_position);
    }

    pub fn ammo(&self) -> u32 {
        self.ammo
    }

    pub fn owner(&self) -> Handle<Actor> {
        self.owner
    }

    pub fn set_owner(&mut self, owner: Handle<Actor>) {
        self.owner = owner;
    }

    pub fn definition(&self) -> &'static WeaponDefinition {
        Self::get_definition(self.kind)
    }

    pub fn try_shoot(&mut self, scene: &mut Scene, time: GameTime) -> bool {
        if self.ammo != 0 && time.elapsed - self.last_shot_time >= self.definition().shoot_interval
        {
            self.ammo -= 1;

            self.offset = Vector3::new(0.0, 0.0, -0.05);
            self.last_shot_time = time.elapsed;

            let position = self.get_shot_position(&scene.graph);

            if let Some(sender) = self.sender.as_ref() {
                sender
                    .send(Message::PlaySound {
                        path: PathBuf::from(self.definition().shot_sound),
                        position,
                        gain: 1.0,
                        rolloff_factor: 5.0,
                        radius: 3.0,
                    })
                    .unwrap();
            }

            true
        } else {
            false
        }
    }

    pub fn clean_up(&mut self, scene: &mut Scene) {
        scene.graph.remove_node(self.model);
        scene.graph.remove_node(self.laser_dot);
    }
}

#[derive(Default, Visit)]
pub struct WeaponContainer {
    pool: Pool<Weapon>,
}

impl WeaponContainer {
    pub fn new() -> Self {
        Self { pool: Pool::new() }
    }

    pub fn add(&mut self, weapon: Weapon) -> Handle<Weapon> {
        self.pool.spawn(weapon)
    }

    pub fn contains(&self, weapon: Handle<Weapon>) -> bool {
        self.pool.is_valid_handle(weapon)
    }

    pub fn free(&mut self, weapon: Handle<Weapon>) {
        self.pool.free(weapon);
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Weapon> {
        self.pool.iter_mut()
    }

    pub fn update(&mut self, scene: &mut Scene, actors: &ActorContainer) {
        for weapon in self.pool.iter_mut() {
            weapon.update(scene, actors)
        }
    }
}

impl Index<Handle<Weapon>> for WeaponContainer {
    type Output = Weapon;

    fn index(&self, index: Handle<Weapon>) -> &Self::Output {
        &self.pool[index]
    }
}

impl IndexMut<Handle<Weapon>> for WeaponContainer {
    fn index_mut(&mut self, index: Handle<Weapon>) -> &mut Self::Output {
        &mut self.pool[index]
    }
}
