use std::{io::Cursor, path::PathBuf};

use crate::commands::{log, LogTarget};

const TAG_XATTR_NAME: &str = "com.apple.metadata:_kMDItemUserTags";
const TAG_XATTR_DROP_ITEM_VALUE: &str = "Dropped\n1";

fn get_file_tags(file_path: &PathBuf) -> Vec<String> {
    let tag_xattr: Option<Vec<u8>> = xattr::get(file_path, TAG_XATTR_NAME.clone()).unwrap();

    match tag_xattr {
        Some(_xattr) => plist::from_bytes::<Vec<String>>(&_xattr).unwrap(),
        None => vec![],
    }
}

fn set_file_xattr_tags(file_path: &PathBuf, tags: &Vec<String>) -> Result<(), std::io::Error> {
    let mut bplist: Vec<u8> = Vec::new();
    let writer = Cursor::new(&mut bplist);
    plist::to_writer_binary(writer, tags).unwrap();

    xattr::set(file_path, TAG_XATTR_NAME.clone(), &bplist)
}

pub fn has_file_drop_attr(file_path: &PathBuf) -> bool {
    return get_file_tags(file_path).contains(&TAG_XATTR_DROP_ITEM_VALUE.to_string());
}

pub async fn set_file_drop_attr(file_path: &PathBuf, log_target: &mut LogTarget<'_>) {
    let mut file_tags = get_file_tags(file_path);
    if !file_tags.contains(&TAG_XATTR_DROP_ITEM_VALUE.to_string()) {
        file_tags.push(TAG_XATTR_DROP_ITEM_VALUE.to_string());
        let set_result = set_file_xattr_tags(file_path, &file_tags);
        log(
            &format!(
                "set-drop {} {}",
                file_path.display(),
                match set_result {
                    Ok(_) => String::from("ok"),
                    Err(err) => format!("not ok ({})", err),
                }
            ),
            log_target,
        )
        .await;
    }
}

pub async fn unset_file_drop_attr(file_path: &PathBuf, log_target: &mut LogTarget<'_>) {
    let file_tags = get_file_tags(file_path);
    if file_tags.contains(&TAG_XATTR_DROP_ITEM_VALUE.to_string()) {
        let set_result = set_file_xattr_tags(
            file_path,
            &file_tags
                .into_iter()
                .filter(|x| false && !x.eq(&TAG_XATTR_DROP_ITEM_VALUE))
                .collect(),
        );
        log(
            &format!(
                "unset-drop {} {}",
                file_path.display(),
                match set_result {
                    Ok(_) => String::from("ok"),
                    Err(err) => format!("not ok ({})", err),
                }
            ),
            log_target,
        )
        .await;
    }
}
