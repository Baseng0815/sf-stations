use std::{collections::HashSet, error::Error, fs, path::Path};

use egui::{emath::RectTransform, pos2, vec2, Color32, DragValue, Pos2, Rect, Sense, Stroke};
use rand::Rng;
use serde::Deserialize;

const MAP_LEFT: f32 = -324600.0;
const MAP_TOP: f32 = -375000.0;
const MAP_RIGHT: f32 = 425300.0;
const MAP_BOT: f32 = 375000.0;

#[allow(non_snake_case)]
#[derive(Debug, Deserialize)]
struct Map {
    pub options: Vec<Resources>,
    pub version: i64,
    pub lastBuild: String,
}

#[allow(non_snake_case)]
#[derive(Debug, Deserialize)]
struct Resources {
    pub tabId: String,
    pub name: String,
    pub options: Vec<ResourceCategory>,
}

#[derive(Debug, Deserialize)]
struct ResourceCategory {
    pub name: String,
    pub r#type: Option<String>,
    pub options: Vec<ResourceLayer>,
}

#[allow(non_snake_case)]
#[derive(Debug, Clone, Deserialize)]
struct ResourceLayer {
    pub layerId: String,
    pub name: String,
    pub purity: Option<String>,
    pub outsideColor: String,
    pub insideColor: String,
    pub icon: String,
    pub markers: Vec<ResourceMarker>,
}

#[allow(non_snake_case)]
#[derive(Debug, Clone, Deserialize)]
struct ResourceMarker {
    pub pathName: String,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub r#type: Option<String>,
    pub purity: String,
    pub obstructed: Option<bool>,
    pub lastCheck: String,
}

impl Map {
    pub fn extract_layers_and_markers(&self) -> (Vec<ResourceLayer>, Vec<ResourceMarker>) {
        let mut markers = Vec::new();
        let mut layers = Vec::new();
        for o0 in &self.options {
            for o1 in &o0.options {
                for o2 in &o1.options {
                    layers.push(o2.clone());
                    for marker in &o2.markers {
                        markers.push(marker.clone());
                    }
                }
            }
        }

        (layers, markers)
    }
}

struct Frontend {
    // rendering and other control stuff
    layers: Vec<ResourceLayer>,
    markers: Vec<ResourceMarker>,
    run_continuously: bool,

    // things that change every run
    points: Vec<Pos2>,
    sets: Vec<Vec<usize>>,
    last_error: f32,
    best_so_far: f32,
    best_so_far_points: Vec<Pos2>,

    // algorithm parameters
    k: usize,
    anneal_step: f32,
    anneal_epsilon: f32,
    k_median_max_iter: u32,
    k_median_epsilon: f32,
}

impl Frontend {
    fn new(layers: Vec<ResourceLayer>, markers: Vec<ResourceMarker>) -> Self {
        Self {
            layers,
            markers,
            run_continuously: false,
            points: Vec::new(),
            sets: Vec::new(),
            last_error: f32::MAX,
            best_so_far: f32::MAX,
            best_so_far_points: Vec::new(),
            k: 10,
            anneal_step: 10000.0,
            anneal_epsilon: 1.0,
            k_median_max_iter: 10,
            k_median_epsilon: 10.0,
        }
    }
}

impl eframe::App for Frontend {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        egui::SidePanel::right("side_panel").show(ctx, |ui| {
            ui.heading("Controls");
            ui.label("k");
            ui.add(DragValue::new(&mut self.k).range(1..=50));
            ui.label("max iterations");
            ui.add(DragValue::new(&mut self.k_median_max_iter).range(1..=100));
            ui.label("anneal epsilon");
            ui.add(DragValue::new(&mut self.anneal_epsilon).range(0.1..=10000.0));
            ui.label("k median epsilon");
            ui.add(DragValue::new(&mut self.k_median_epsilon).range(0.1..=10000.0));

            ui.separator();

            if ui.button("reset and run").clicked() {
                self.reinitialize();
                self.run_k_median();
            }

            if ui.button("reset").clicked() {
                self.reinitialize();
            }

            if ui.button("step once").clicked() {
                if self.sets.len() != self.k {
                    self.reinitialize();
                }
                self.run_k_median();
            }

            ui.checkbox(&mut self.run_continuously, "run continuously");
            if self.run_continuously {
                self.reinitialize();
                self.run_k_median();
            }

            ui.separator();

            ui.heading("Information");
            ui.label(format!("Last run total error: {}", self.last_error));
            ui.label(format!("Best so far: {}", self.best_so_far));
            ui.label(format!("Best so far points: {:#?}", self.best_so_far_points));
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::drag());
            let to_screen = RectTransform::from_to(
                Rect::from_min_max(pos2(MAP_LEFT, MAP_TOP), pos2(MAP_RIGHT, MAP_BOT)),
                response.rect,
            );
            for layer in &self.layers {
                for marker in &layer.markers {
                    // find closest_distance (for coloring)
                    let mut closest_distance = f32::MAX;
                    let mut closest_index = 0;
                    for (i, point) in self.points.iter().enumerate() {
                        let distance = (*point - pos2(marker.x, marker.y)).length();
                        if distance < closest_distance {
                            closest_distance = distance;
                            closest_index = i;
                        }
                    }

                    let color = Color32::from_rgb(
                        (255.0 / self.k as f32 * closest_index as f32) as u8,
                        128,
                        196,
                    );
                    let pos = to_screen.transform_pos(pos2(marker.x, marker.y));
                    painter.circle(pos, 3.0, color, Stroke::default());
                }
            }

            for point in &self.points {
                let pos = to_screen.transform_pos(*point);
                painter.circle(pos, 8.0, Color32::GREEN, Stroke::default());
            }
        });
    }
}

impl Frontend {
    fn reinitialize(&mut self) {
        // initialization strategy: random (surely this will be fine)
        self.points = (0..self.k)
            .map(|_| {
                let x = rand::thread_rng().gen_range(MAP_LEFT..MAP_RIGHT);
                let y = rand::thread_rng().gen_range(MAP_TOP..MAP_BOT);
                pos2(x, y)
            })
        .collect::<Vec<_>>();

        self.sets = vec![Vec::new(); self.k];
    }

    fn simulated_annealing(&self, indices: &Vec<usize>) -> Pos2 {
        let directions = vec![
            vec2(1.0, 0.0),
            vec2(-1.0, 0.0),
            vec2(0.0, 1.0),
            vec2(0.0, -1.0),
        ];

        let mut step = self.anneal_step;
        let mut median = indices
            .iter()
            .map(|i| vec2(self.markers[*i].x, self.markers[*i].y))
            .fold(Pos2::ZERO, |acc, v| acc + v);
        median.x /= indices.len() as f32;
        median.y /= indices.len() as f32;

        let mut min: f32 = indices
            .iter()
            .map(|i| {
                ((self.markers[*i].x - median.x).powi(2) + (self.markers[*i].y - median.y).powi(2))
                    .sqrt()
            })
        .sum();
        let mut improved = false;

        while step > self.anneal_epsilon {
            for direction in &directions {
                let temp_median = median + step * *direction;
                let d = indices
                    .iter()
                    .map(|i| {
                        ((self.markers[*i].x - median.x).powi(2)
                         + (self.markers[*i].y - median.y).powi(2))
                            .sqrt()
                    })
                .sum();
                if d < min {
                    min = d;
                    median = temp_median;
                    improved = true;
                    break;
                }
            }

            if !improved {
                step *= 0.5;
            }
        }

        median
    }

    fn run_k_median(&mut self) {
        for _ in 0..self.k_median_max_iter {
            // clear previous sets
            for set in self.sets.iter_mut() {
                set.clear();
            }

            // partition markers into disjoint sets based on the closest point to them
            for (markeri, marker) in self.markers.iter().enumerate() {
                let mut closest_distance = f32::MAX;
                let mut closest_index = 0;
                for (i, point) in self.points.iter().enumerate() {
                    let distance = (*point - pos2(marker.x, marker.y)).length();
                    if distance < closest_distance {
                        closest_distance = distance;
                        closest_index = i;
                    }
                }
                self.sets[closest_index].push(markeri);
            }

            // calculate median for each set and adjust points accordingly
            for (seti, set) in self.sets.iter().enumerate() {
                let median = self.simulated_annealing(&set);
                self.points[seti] = median;
            }

            // find new error values and abort if threshold reached
            let total_error: f32 = self
                .points
                .iter()
                .zip(self.sets.iter())
                .map(|(point, set)| {
                    let set_error: f32 = set
                        .iter()
                        .map(|i| {
                            ((self.markers[*i].x - point.x).powi(2)
                             + (self.markers[*i].y - point.y).powi(2))
                                .sqrt()
                        })
                    .sum();
                    set_error
                })
            .sum();

            if total_error < self.best_so_far {
                self.best_so_far = total_error;
                self.best_so_far_points = self.points.clone();
            }

            if (total_error - self.last_error).abs() < self.k_median_epsilon as f32 {
                self.last_error =  total_error;
                break;
            }

            self.last_error =  total_error;
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let map_content = fs::read_to_string(Path::new("./assets/map_data.json"))?;
    let map: Map = serde_json::from_str(&map_content)?;

    let (layers, markers) = map.extract_layers_and_markers();
    eprintln!("layers = {:#?}", layers);

    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        "Satisfactory station planner",
        native_options,
        Box::new(|cc| Ok(Box::new(Frontend::new(layers, markers)))),
    )
        .unwrap();

    Ok(())
}
