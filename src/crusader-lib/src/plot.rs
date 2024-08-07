use anyhow::anyhow;
use image::{ImageBuffer, ImageFormat, Rgb};
use plotters::coord::types::RangedCoordf64;
use plotters::coord::Shift;
use plotters::prelude::*;
use plotters::style::text_anchor::{HPos, Pos, VPos};
use plotters::style::{register_font, RGBColor};

use std::mem;
use std::path::Path;
use std::time::Duration;

use crate::file_format::{RawPing, RawResult};
use crate::protocol::RawLatency;
use crate::test::{unique, PlotConfig};

const UP_COLOR: RGBColor = RGBColor(37, 83, 169);
const DOWN_COLOR: RGBColor = RGBColor(95, 145, 62);

pub fn register_fonts() {
    register_font(
        "sans-serif",
        FontStyle::Normal,
        include_bytes!("../Ubuntu-Light.ttf"),
    )
    .map_err(|_| ())
    .unwrap();
}

impl RawResult {
    pub fn to_test_result(&self) -> TestResult {
        let throughput_interval = self.config.bandwidth_interval;

        let stream_groups: Vec<_> = self
            .stream_groups
            .iter()
            .map(|group| TestStreamGroup {
                download: group.download,
                both: group.both,
                streams: (0..(group.streams.len()))
                    .map(|i| {
                        let bytes: Vec<_> = (0..=i)
                            .map(|i| to_float(&group.streams[i].to_vec()))
                            .collect();
                        let bytes: Vec<_> = bytes.iter().map(|stream| stream.as_slice()).collect();
                        TestStream {
                            data: sum_bytes(&bytes, throughput_interval),
                        }
                    })
                    .collect(),
            })
            .collect();

        let process_bytes = |bytes: Vec<Vec<(u64, u64)>>| -> Vec<(u64, f64)> {
            let bytes: Vec<_> = bytes.iter().map(|stream| to_float(stream)).collect();
            let bytes: Vec<_> = bytes.iter().map(|stream| stream.as_slice()).collect();
            sum_bytes(&bytes, throughput_interval)
        };

        let groups: Vec<_> = self
            .stream_groups
            .iter()
            .map(|group| {
                let streams: Vec<_> = group.streams.iter().map(|stream| stream.to_vec()).collect();
                let single = process_bytes(streams);
                (group, single)
            })
            .collect();

        let find = |download, both| {
            groups
                .iter()
                .find(|group| group.0.download == download && group.0.both == both)
                .map(|group| group.1.clone())
        };

        let download_bytes_sum = find(true, false);
        let both_download_bytes_sum = find(true, true);

        let combined_download_bytes: Vec<_> = [
            download_bytes_sum.as_deref(),
            both_download_bytes_sum.as_deref(),
        ]
        .into_iter()
        .flatten()
        .collect();
        let combined_download_bytes = sum_bytes(&combined_download_bytes, throughput_interval);

        let upload_bytes_sum = find(false, false);

        let both_upload_bytes_sum = find(false, true);

        let combined_upload_bytes: Vec<_> = [
            upload_bytes_sum.as_deref(),
            both_upload_bytes_sum.as_deref(),
        ]
        .into_iter()
        .flatten()
        .collect();
        let combined_upload_bytes = sum_bytes(&combined_upload_bytes, throughput_interval);

        let both_bytes = self.both().then(|| {
            sum_bytes(
                &[
                    both_download_bytes_sum.as_deref().unwrap(),
                    both_upload_bytes_sum.as_deref().unwrap(),
                ],
                throughput_interval,
            )
        });

        let pings = self.pings.clone();

        TestResult {
            raw_result: self.clone(),
            start: self.start,
            duration: self.duration,
            pings,
            both_bytes,
            both_download_bytes: both_download_bytes_sum,
            both_upload_bytes: both_upload_bytes_sum,
            download_bytes: download_bytes_sum,
            upload_bytes: upload_bytes_sum,
            combined_download_bytes,
            combined_upload_bytes,
            stream_groups,
        }
    }
}

pub struct TestStream {
    pub data: Vec<(u64, f64)>,
}

pub struct TestStreamGroup {
    pub download: bool,
    pub both: bool,
    pub streams: Vec<TestStream>,
}

pub struct TestResult {
    pub raw_result: RawResult,
    pub start: Duration,
    pub duration: Duration,
    pub download_bytes: Option<Vec<(u64, f64)>>,
    pub upload_bytes: Option<Vec<(u64, f64)>>,
    pub combined_download_bytes: Vec<(u64, f64)>,
    pub combined_upload_bytes: Vec<(u64, f64)>,
    pub both_download_bytes: Option<Vec<(u64, f64)>>,
    pub both_upload_bytes: Option<Vec<(u64, f64)>>,
    pub both_bytes: Option<Vec<(u64, f64)>>,
    pub pings: Vec<RawPing>,
    pub stream_groups: Vec<TestStreamGroup>,
}

pub fn save_graph(config: &PlotConfig, result: &TestResult, name: &str) -> String {
    let file = unique(name, "png");
    save_graph_to_path(file.as_ref(), config, result);
    file
}

pub fn save_graph_to_path(path: &Path, config: &PlotConfig, result: &TestResult) {
    let img = save_graph_to_mem(config, result).expect("Unable to write plot to file");
    img.save_with_format(&path, ImageFormat::Png)
        .expect("Unable to write plot to file");
}

pub(crate) fn save_graph_to_mem(
    config: &PlotConfig,
    result: &TestResult,
) -> Result<ImageBuffer<Rgb<u8>, Vec<u8>>, anyhow::Error> {
    let mut throughput = Vec::new();

    result.both_bytes.as_ref().map(|both_bytes| {
        throughput.push((
            "Both",
            RGBColor(149, 96, 153),
            to_rates(both_bytes),
            vec![both_bytes.as_slice()],
        ));
    });

    if result.upload_bytes.is_some() || result.both_upload_bytes.is_some() {
        throughput.push((
            "Upload",
            UP_COLOR,
            to_rates(&result.combined_upload_bytes),
            [
                result.upload_bytes.as_deref(),
                result.both_upload_bytes.as_deref(),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>(),
        ));
    }

    if result.download_bytes.is_some() || result.both_download_bytes.is_some() {
        throughput.push((
            "Download",
            DOWN_COLOR,
            to_rates(&result.combined_download_bytes),
            [
                result.download_bytes.as_deref(),
                result.both_download_bytes.as_deref(),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>(),
        ));
    }

    graph(
        config,
        result,
        &result.pings,
        &throughput,
        result.start.as_secs_f64(),
        result.duration.as_secs_f64(),
    )
}

pub fn float_max(iter: impl Iterator<Item = f64>) -> f64 {
    let mut max = iter.fold(f64::NAN, f64::max);

    if max.is_nan() {
        max = 100.0;
    }

    max
}

fn to_float(stream: &[(u64, u64)]) -> Vec<(u64, f64)> {
    stream.iter().map(|(t, v)| (*t, *v as f64)).collect()
}

pub fn to_rates(stream: &[(u64, f64)]) -> Vec<(u64, f64)> {
    let mut result: Vec<(u64, f64)> = (0..stream.len())
        .map(|i| {
            let rate = if i > 0 {
                let bytes = stream[i].1 - stream[i - 1].1;
                let duration = Duration::from_micros(stream[i].0 - stream[i - 1].0);
                let mbits = (bytes * 8.0) / (1000.0 * 1000.0);
                mbits / duration.as_secs_f64()
            } else {
                0.0
            };
            (stream[i].0, rate)
        })
        .collect();

    // Insert dummy zero points for nicer graphs
    if !result.is_empty() {
        result.first().unwrap().0.checked_sub(1).map(|first| {
            result.insert(0, (first, 0.0));
        });
        result.push((result.last().unwrap().0 + 1, 0.0));
    }

    result
}

fn sum_bytes(input: &[&[(u64, f64)]], interval: Duration) -> Vec<(u64, f64)> {
    let interval = interval.as_micros() as u64;

    let throughput: Vec<_> = input
        .iter()
        .map(|stream| interpolate(stream, interval))
        .collect();

    let min = throughput
        .iter()
        .map(|stream| stream.first().map(|e| e.0).unwrap_or(0))
        .min()
        .unwrap_or(0);

    let max = throughput
        .iter()
        .map(|stream| stream.last().map(|e| e.0).unwrap_or(0))
        .max()
        .unwrap_or(0);

    let mut data = Vec::new();

    for point in (min..=max).step_by(interval as usize) {
        let value = throughput
            .iter()
            .map(
                |stream| match stream.binary_search_by_key(&point, |e| e.0) {
                    Ok(i) => stream[i].1,
                    Err(0) => 0.0,
                    Err(i) if i == stream.len() => stream.last().unwrap().1,
                    _ => panic!("unexpected index"),
                },
            )
            .sum();
        data.push((point, value));
    }

    data
}

fn interpolate(input: &[(u64, f64)], interval: u64) -> Vec<(u64, f64)> {
    if input.is_empty() {
        return Vec::new();
    }

    let min = input.first().unwrap().0 / interval * interval;
    let max = (input.last().unwrap().0 + interval - 1) / interval * interval;

    let mut data = Vec::new();

    for point in (min..=max).step_by(interval as usize) {
        let i = input.partition_point(|e| e.0 < point);
        let value = if i == input.len() {
            input.last().unwrap().1
        } else if input[i].0 == point || i == 0 {
            input[i].1
        } else {
            let len = input[i].0 - input[i - 1].0;
            if len == 0 {
                input[i].1
            } else {
                let ratio = (point - input[i - 1].0) as f64 / len as f64;
                let delta = input[i].1 - input[i - 1].1;
                input[i - 1].1 + delta * ratio
            }
        };
        data.push((point, value));
    }

    data
}

fn new_chart<'a, 'b, 'c>(
    duration: f64,
    padding_bottom: Option<i32>,
    max: f64,
    label: &'b str,
    x_label: Option<&'b str>,
    area: &'a DrawingArea<BitMapBackend<'c>, Shift>,
) -> ChartContext<'a, BitMapBackend<'c>, Cartesian2d<RangedCoordf64, RangedCoordf64>> {
    let font = (FontFamily::SansSerif, 16);

    let mut chart = ChartBuilder::on(area)
        .margin(6)
        .set_label_area_size(LabelAreaPosition::Left, 100)
        .set_label_area_size(LabelAreaPosition::Right, 100)
        .set_label_area_size(LabelAreaPosition::Bottom, padding_bottom.unwrap_or(20))
        .build_cartesian_2d(0.0..duration, 0.0..max)
        .unwrap();

    chart
        .plotting_area()
        .fill(&RGBColor(248, 248, 248))
        .unwrap();

    let mut mesh = chart.configure_mesh();

    mesh.disable_x_mesh().disable_y_mesh();

    if x_label.is_none() {
        mesh.x_labels(20).y_labels(10);
    } else {
        mesh.x_labels(0).y_labels(0);
    }

    mesh.x_label_style(font).y_label_style(font).y_desc(label);

    if let Some(label) = x_label {
        mesh.x_desc(label);
    }

    mesh.draw().unwrap();

    chart
}

fn legends<'a, 'b: 'a>(
    chart: &mut ChartContext<'a, BitMapBackend<'b>, Cartesian2d<RangedCoordf64, RangedCoordf64>>,
) {
    let font = (FontFamily::SansSerif, 16);

    chart
        .configure_series_labels()
        .background_style(WHITE.mix(0.8))
        .label_font(font)
        .border_style(BLACK)
        .draw()
        .unwrap();
}

fn latency<'a>(
    config: &PlotConfig,
    result: &TestResult,
    pings: &[RawPing],
    start: f64,
    duration: f64,
    area: &DrawingArea<BitMapBackend<'a>, Shift>,
    packet_loss_area: Option<&DrawingArea<BitMapBackend<'a>, Shift>>,
    peer: bool,
) {
    let new_area;
    let new_packet_loss_area;
    let (packet_loss_area, area) = if let Some(packet_loss_area) = packet_loss_area {
        (packet_loss_area, area)
    } else {
        (new_area, new_packet_loss_area) =
            area.split_vertically(area.relative_to_height(1.0) - 70.0);
        (&new_packet_loss_area, &new_area)
    };

    let max_latency = pings
        .iter()
        .filter_map(|d| d.latency)
        .filter_map(|latency| latency.total)
        .max()
        .unwrap_or(Duration::from_millis(100))
        .as_secs_f64()
        * 1000.0;

    let mut max_latency = max_latency * 1.05;

    if let Some(max) = config.max_latency.map(|l| l as f64) {
        if max > max_latency {
            max_latency = max;
        }
    }

    // Latency

    let mut chart = new_chart(
        duration,
        None,
        max_latency,
        if peer {
            "Peer latency (ms)"
        } else {
            "Latency (ms)"
        },
        None,
        area,
    );

    let mut draw_latency =
        |color: RGBColor, name: &str, get_latency: fn(&RawLatency) -> Option<Duration>| {
            let mut data = Vec::new();

            let flush = |data: &mut Vec<_>| {
                let data = mem::take(data);

                if data.len() == 1 {
                    chart
                        .plotting_area()
                        .draw(&Circle::new(data[0], 1, color.filled()))
                        .unwrap();
                } else {
                    chart
                        .plotting_area()
                        .draw(&PathElement::new(data, color))
                        .unwrap();
                }
            };

            for ping in pings {
                match &ping.latency {
                    Some(latency) => match get_latency(latency) {
                        Some(latency) => {
                            let x = ping.sent.as_secs_f64() - start;
                            let y = latency.as_secs_f64() * 1000.0;

                            data.push((x, y));
                        }
                        None => {
                            flush(&mut data);
                        }
                    },
                    None => {
                        flush(&mut data);
                    }
                }
            }

            flush(&mut data);

            chart
                .draw_series(LineSeries::new(std::iter::empty(), color))
                .unwrap()
                .label(name)
                .legend(move |(x, y)| {
                    Rectangle::new([(x, y - 5), (x + 18, y + 3)], color.filled())
                });
        };

    draw_latency(UP_COLOR, "Up", |latency| Some(latency.up));

    draw_latency(DOWN_COLOR, "Down", |latency| latency.down());

    draw_latency(RGBColor(50, 50, 50), "Total", |latency| latency.total);

    legends(&mut chart);

    // Packet loss

    let chart = new_chart(
        duration,
        Some(30),
        1.0,
        if peer { "Peer loss" } else { "Packet loss" },
        Some("Elapsed time (seconds)"),
        packet_loss_area,
    );

    for ping in pings {
        let x = ping.sent.as_secs_f64() - start;
        if ping.latency.and_then(|latency| latency.total).is_none() {
            let bold_size = 0.1111;
            let (color, s, e, bold) = if result.raw_result.version >= 2 {
                if ping.latency.is_none() {
                    (UP_COLOR, 0.0, 0.5, Some(0.0 + bold_size))
                } else {
                    (DOWN_COLOR, 1.0, 0.5, Some(1.0 - bold_size))
                }
            } else {
                (RGBColor(193, 85, 85), 0.0, 1.0, None)
            };
            chart
                .plotting_area()
                .draw(&PathElement::new(vec![(x, s), (x, e)], color))
                .unwrap();
            bold.map(|bold| {
                chart
                    .plotting_area()
                    .draw(&PathElement::new(
                        vec![(x, s), (x, bold)],
                        color.stroke_width(2),
                    ))
                    .unwrap();
            });
        }
    }

    chart
        .plotting_area()
        .draw(&PathElement::new(vec![(0.0, 1.0), (duration, 1.0)], BLACK))
        .unwrap();
}

fn plot_split_throughput(
    config: &PlotConfig,
    download: bool,
    result: &TestResult,
    start: f64,
    duration: f64,
    area: &DrawingArea<BitMapBackend, Shift>,
) {
    let groups: Vec<_> = result
        .stream_groups
        .iter()
        .filter(|group| group.download == download)
        .map(|group| TestStreamGroup {
            download,
            both: group.both,
            streams: group
                .streams
                .iter()
                .map(|stream| TestStream {
                    data: to_rates(&stream.data),
                })
                .collect(),
        })
        .collect();

    let max_throughput = float_max(
        groups
            .iter()
            .flat_map(|group| group.streams.last().unwrap().data.iter())
            .map(|e| e.1),
    );

    let mut max_throughput = max_throughput * 1.05;

    if let Some(max) = config.max_throughput.map(|l| l as f64 / (1000.0 * 1000.0)) {
        if max > max_throughput {
            max_throughput = max;
        }
    }

    let mut chart = new_chart(
        duration,
        None,
        max_throughput,
        if download {
            "Download (Mbps)"
        } else {
            "Upload (Mbps)"
        },
        None,
        area,
    );

    for group in groups {
        for i in 0..(group.streams.len()) {
            let main = i == group.streams.len() - 1;
            let color = if download {
                if main {
                    DOWN_COLOR
                } else {
                    if i & 1 == 0 {
                        RGBColor(188, 203, 177)
                    } else {
                        RGBColor(215, 223, 208)
                    }
                }
            } else {
                if main {
                    UP_COLOR
                } else {
                    if i & 1 == 0 {
                        RGBColor(159, 172, 202)
                    } else {
                        RGBColor(211, 217, 231)
                    }
                }
            };
            chart
                .draw_series(LineSeries::new(
                    group.streams[i].data.iter().map(|(time, rate)| {
                        (Duration::from_micros(*time).as_secs_f64() - start, *rate)
                    }),
                    color,
                ))
                .unwrap();
        }
    }
}

fn plot_throughput(
    config: &PlotConfig,
    throughput: &[(&str, RGBColor, Vec<(u64, f64)>, Vec<&[(u64, f64)]>)],
    start: f64,
    duration: f64,
    area: &DrawingArea<BitMapBackend, Shift>,
) {
    let max_throughput = float_max(
        throughput
            .iter()
            .flat_map(|list| list.2.iter())
            .map(|e| e.1),
    );

    let mut max_throughput = max_throughput * 1.05;

    if let Some(max) = config.max_throughput.map(|l| l as f64 / (1000.0 * 1000.0)) {
        if max > max_throughput {
            max_throughput = max;
        }
    }

    let mut chart = new_chart(
        duration,
        None,
        max_throughput,
        "Throughput (Mbps)",
        None,
        area,
    );

    for (name, color, rates, _) in throughput {
        chart
            .draw_series(LineSeries::new(
                rates.iter().map(|(time, rate)| {
                    (Duration::from_micros(*time).as_secs_f64() - start, *rate)
                }),
                color,
            ))
            .unwrap()
            .label(*name)
            .legend(move |(x, y)| Rectangle::new([(x, y - 5), (x + 18, y + 3)], color.filled()));
    }

    legends(&mut chart);
}

pub(crate) fn bytes_transferred(
    throughput: &[(&str, RGBColor, Vec<(u64, f64)>, Vec<&[(u64, f64)]>)],
    start: f64,
    duration: f64,
    area: &DrawingArea<BitMapBackend, Shift>,
) {
    let max_bytes = float_max(
        throughput
            .iter()
            .flat_map(|list| list.3.iter())
            .flat_map(|list| list.iter())
            .map(|e| e.1),
    );

    let max_bytes = max_bytes / (1024.0 * 1024.0 * 1024.0);

    let max_bytes = max_bytes * 1.05;

    let mut chart = new_chart(
        duration,
        Some(50),
        max_bytes,
        "Data transferred (GiB)",
        None,
        area,
    );

    for (name, color, _, bytes) in throughput {
        for (i, bytes) in bytes.iter().enumerate() {
            let series = chart
                .draw_series(LineSeries::new(
                    bytes.iter().map(|(time, bytes)| {
                        (
                            Duration::from_micros(*time).as_secs_f64() - start,
                            *bytes / (1024.0 * 1024.0 * 1024.0),
                        )
                    }),
                    &color,
                ))
                .unwrap();

            if i == 0 {
                series.label(*name).legend(move |(x, y)| {
                    Rectangle::new([(x, y - 5), (x + 18, y + 3)], color.filled())
                });
            }
        }
    }

    legends(&mut chart);
}

pub(crate) fn graph(
    config: &PlotConfig,
    result: &TestResult,
    pings: &[RawPing],
    throughput: &[(&str, RGBColor, Vec<(u64, f64)>, Vec<&[(u64, f64)]>)],
    start: f64,
    duration: f64,
) -> Result<ImageBuffer<Rgb<u8>, Vec<u8>>, anyhow::Error> {
    let width = config.width.unwrap_or(1280) as u32;

    let peer_latency = result.raw_result.peer_pings.is_some();

    let mut def_height = 720;

    if peer_latency {
        def_height += 380;
    }

    let height = config.height.unwrap_or(def_height) as u32;

    let mut data = vec![0; 3 * (width as usize * height as usize)];

    let title = config.title.as_deref().unwrap_or("Latency under load");

    {
        let root = BitMapBackend::with_buffer(&mut data, (width, height)).into_drawing_area();

        root.fill(&WHITE).unwrap();

        let style: TextStyle = (FontFamily::SansSerif, 26).into();

        let small_style: TextStyle = (FontFamily::SansSerif, 14).into();

        let lines = 2;

        let text_height =
            (root.estimate_text_size("Wg", &small_style).unwrap().1 as i32 + 5) * lines;

        let center = text_height / 2 + 10;

        root.draw_text(
            title,
            &style.pos(Pos::new(HPos::Center, VPos::Center)),
            (width as i32 / 2, center),
        )
        .unwrap();

        if result.raw_result.version >= 1 {
            let top_margin = 10;
            root.draw_text(
                &format!(
                    "Connections: {} over IPv{}",
                    result.raw_result.streams(),
                    if result.raw_result.ipv6 { 6 } else { 4 },
                ),
                &small_style.pos(Pos::new(HPos::Left, VPos::Top)),
                (100, top_margin + text_height / lines),
            )
            .unwrap();

            root.draw_text(
                &format!(
                    "Stagger: {} s",
                    result.raw_result.config.stagger.as_secs_f64(),
                ),
                &small_style.pos(Pos::new(HPos::Left, VPos::Top)),
                (100 + 180, top_margin + text_height / lines),
            )
            .unwrap();

            root.draw_text(
                &format!(
                    "Load duration: {:.2} s",
                    result.raw_result.config.load_duration.as_secs_f64(),
                ),
                &small_style.pos(Pos::new(HPos::Left, VPos::Top)),
                (100, top_margin),
            )
            .unwrap();

            root.draw_text(
                &format!(
                    "Server latency: {:.2} ms",
                    result.raw_result.server_latency.as_secs_f64() * 1000.0,
                ),
                &small_style.pos(Pos::new(HPos::Left, VPos::Top)),
                (100 + 180, top_margin),
            )
            .unwrap();

            root.draw_text(
                &result.raw_result.generated_by,
                &small_style.pos(Pos::new(HPos::Right, VPos::Center)),
                (width as i32 - 100, center),
            )
            .unwrap();
        }

        let mut root = root.split_vertically(text_height + 10).1;

        let loss = if !peer_latency {
            let loss;
            (root, loss) = root.split_vertically(root.relative_to_height(1.0) - 70.0);
            Some(loss)
        } else {
            None
        };

        let mut charts = 1;

        if peer_latency {
            charts += 1;
        }

        if result.raw_result.streams() > 0 {
            if config.split_throughput {
                if result.raw_result.download() || result.raw_result.both() {
                    charts += 1
                }
                if result.raw_result.upload() || result.raw_result.both() {
                    charts += 1
                }
            } else {
                charts += 1
            }
            if config.transferred {
                charts += 1
            }
        }

        let areas = root.split_evenly((charts, 1));

        // Scale to fit the legend
        let duration = duration * 1.08;

        let mut chart_index = 0;

        if result.raw_result.streams() > 0 {
            if config.split_throughput {
                if result.raw_result.download() || result.raw_result.both() {
                    plot_split_throughput(
                        config,
                        true,
                        result,
                        start,
                        duration,
                        &areas[chart_index],
                    );
                    chart_index += 1;
                }
                if result.raw_result.upload() || result.raw_result.both() {
                    plot_split_throughput(
                        config,
                        false,
                        result,
                        start,
                        duration,
                        &areas[chart_index],
                    );
                    chart_index += 1;
                }
            } else {
                plot_throughput(config, throughput, start, duration, &areas[chart_index]);
                chart_index += 1;
            }
        }

        latency(
            config,
            result,
            pings,
            start,
            duration,
            &areas[chart_index],
            loss.as_ref(),
            false,
        );
        chart_index += 1;

        if let Some(peer_pings) = result.raw_result.peer_pings.as_ref() {
            latency(
                config,
                result,
                peer_pings,
                start,
                duration,
                &areas[chart_index],
                None,
                true,
            );
            chart_index += 1;
        }

        if result.raw_result.streams() > 0 && config.transferred {
            bytes_transferred(throughput, start, duration, &areas[chart_index]);
            #[allow(unused_assignments)]
            {
                chart_index += 1;
            }
        }

        root.present().map_err(|_| anyhow!("Unable to plot"))?;
    }

    ImageBuffer::from_raw(width, height, data).ok_or(anyhow!("Failed to create image"))
}
