//! Coordinate carriers and the dimension rules (`docs/geometry.md`,
//! "Coordinates").

use xeibe_geom::model::Dim;
use xeibe_geom::parse::{CoordinatesFormat, parse_coordinates, parse_pos_list};

use crate::support::coords_to_vec;

#[test]
fn pos_list_is_read_in_the_given_dimension() {
    let coords = parse_pos_list("0 0 1 1 2 2", 2, None).expect("2D positions");
    assert_eq!(coords.dim, Some(Dim::Xy));
    assert_eq!(coords_to_vec(&coords), [[0.0, 0.0], [1.0, 1.0], [2.0, 2.0]]);

    let coords = parse_pos_list("1 2 3 4 5 6", 3, None).expect("3D positions");
    assert_eq!(coords.dim, Some(Dim::Xyz));
    assert_eq!(coords_to_vec(&coords), [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]]);
}

#[test]
fn pos_list_accepts_any_whitespace_and_number_form() {
    let coords = parse_pos_list("\n  1e3\t-2.5  \n +3 4.0e-1\n", 2, None).expect("numbers");
    assert_eq!(coords_to_vec(&coords), [[1000.0, -2.5], [3.0, 0.4]]);
}

#[test]
fn pos_list_checks_the_count_attribute() {
    // `count` is the number of positions (07-036 §10.1.4.2).
    let coords = parse_pos_list("0 0 1 1", 2, Some(2)).expect("two positions");
    assert_eq!(coords_to_vec(&coords).len(), 2);
    assert!(
        parse_pos_list("0 0 1 1", 2, Some(3)).is_err(),
        "count must match the number of values"
    );
}

#[test]
fn a_wrong_number_of_values_is_an_error() {
    assert!(parse_pos_list("0 0 1", 2, None).is_err());
    assert!(parse_pos_list("0 0 1 1 2", 3, None).is_err());
    assert!(parse_pos_list("0 x", 2, None).is_err(), "not a number");
}

#[test]
fn an_empty_pos_list_is_an_empty_coordinate_sequence() {
    let coords = parse_pos_list("   ", 2, None).expect("no positions");
    assert!(coords.values.is_empty());
}

#[test]
fn coordinates_use_the_gml_2_default_separators() {
    // decimal ".", cs ",", ts " " (02-069 §4.3.1).
    let format = CoordinatesFormat::default();
    assert_eq!(format, CoordinatesFormat { decimal: ".".into(), cs: ",".into(), ts: " ".into() });

    let coords = parse_coordinates("1,2 3,4", &format).expect("two positions");
    assert_eq!(coords_to_vec(&coords), [[1.0, 2.0], [3.0, 4.0]]);
    assert_eq!(coords.dim, Some(Dim::Xy));

    let coords = parse_coordinates("1,2,3 4,5,6", &format).expect("3D positions");
    assert_eq!(coords.dim, Some(Dim::Xyz));
    assert_eq!(coords_to_vec(&coords), [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]]);
}

#[test]
fn whitespace_between_tuples_is_read_leniently() {
    // Real files wrap `coordinates` over several lines.
    let coords = parse_coordinates("1,2\n\t 3,4  5,6 ", &CoordinatesFormat::default())
        .expect("three positions");
    assert_eq!(coords_to_vec(&coords), [[1.0, 2.0], [3.0, 4.0], [5.0, 6.0]]);
}

#[test]
fn coordinates_honour_custom_separators() {
    let format = CoordinatesFormat {
        decimal: ",".into(),
        cs: ";".into(),
        ts: " ".into(),
    };
    let coords = parse_coordinates("1,5;2,5 3,0;4,0", &format).expect("two positions");
    assert_eq!(coords_to_vec(&coords), [[1.5, 2.5], [3.0, 4.0]]);
}

#[test]
fn a_corrupt_coordinates_value_is_an_error() {
    let format = CoordinatesFormat::default();
    assert!(parse_coordinates("0", &format).is_err(), "one ordinate");
    assert!(parse_coordinates("1bla2", &format).is_err());
}
