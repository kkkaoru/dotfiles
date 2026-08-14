use super::*;

#[test]
fn forwards_the_inner_body_size_hint() {
    let stream = ActiveBodyStream {
        inner: Body::from(Bytes::from_static(b"hello")).into_data_stream(),
        active: None,
    };

    assert_eq!(stream.size_hint(), (5, Some(5)));
}
