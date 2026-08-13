use fjall::Slice;
use migrateable::{HashedTypeDef, JsonType, KeyType, MigrateResult, Migrateable, RkyvType, Type};
use rkyv::rancor;

#[derive(serde::Serialize, serde::Deserialize, HashedTypeDef)]
pub struct TestStructV1 {
    field: String,
}
impl TestStructV1 {
    pub fn new(field: String) -> Self {
        Self {
            field,
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, HashedTypeDef)]
pub struct TestStructV2 {
    field1: String,
    field2: String,
}
impl TestStructV2 {
    pub fn new(field1: String, field2: String) -> Self {
        Self {
            field1,
            field2,
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, HashedTypeDef)]
pub struct TestStructV3 {
    field1: String,
    field2: String,
    field3: String,
}
impl TestStructV3 {
    pub fn new(field1: String, field2: String, field3: String) -> Self {
        Self {
            field1,
            field2,
            field3,
        }
    }
}

#[derive(rkyv::Archive, rkyv::Deserialize, rkyv::Serialize, HashedTypeDef)]
pub struct RkyvFile {
    pub file_name: String,
    pub data: Vec<u8>,
}
#[derive(HashedTypeDef, migrateable::Locked, storekey::Encode, storekey::BorrowDecode, fjall_expanded::SpaceKey)]
pub struct ImageKeyV1<'a> {
    pub page_id: [u8; 16],
    pub image_file_hash: u64,
    pub image_file_name: std::borrow::Cow<'a, str>,
}

#[derive(migrateable_derive::Migrateable)]
pub enum RkyvDataVersionFromU8 {
    V1(KeyType<ImageKeyV1<'static>>),
}

#[derive(migrateable_derive::Migrateable)]
pub enum RkyvDataVersion {
    V1(Type<Vec<u8>>),
    V2(RkyvType<RkyvFile>),
}

impl MigrateToCurrentRkyvDataVersion for Vec<u8> {
    fn migrate(mut self, version: MigrateableRkyvDataVersion) -> MigrateResult<CurrentRkyvDataVersion> {
        match version {
            MigrateableRkyvDataVersion::V1 => Ok({
                self.insert(0, 1u8);
                RkyvFile {
                    file_name: String::from("amogus.txt"),
                    data: self,
                }
            }),
        }
    }
}

#[derive(migrateable_derive::Migrateable)]
pub enum JsonDataVersion {
    V1(JsonType<TestStructV1>),
    V2(JsonType<TestStructV2>),
    V3(JsonType<TestStructV3>),
}

impl MigrateToCurrentJsonDataVersion for TestStructV1 {
    fn migrate(self, _version: MigrateableJsonDataVersion) -> MigrateResult<CurrentJsonDataVersion> {
        Ok(TestStructV3::new(self.field, String::from("Field2"), String::from("Field3")))
    }
}
impl MigrateToCurrentJsonDataVersion for TestStructV2 {
    fn migrate(self, _version: MigrateableJsonDataVersion) -> MigrateResult<CurrentJsonDataVersion> {
        Ok(TestStructV3::new(self.field1, self.field2, String::from("Field3")))
    }
}

#[test]
fn test_migrations() {
    let version_id = 1u8;
    let file = vec![34u8, 35u8, 36u8];

    let migrated: Vec<u8> = RkyvDataVersion::from_u8(version_id).expect("valid version").migrate(file).expect("migration failed");

    // Check that migration added the version byte (1) at the start
    assert_eq!(
        unsafe { rkyv::from_bytes_unchecked::<CurrentRkyvDataVersion, rancor::Error>(&migrated) }
            .expect("vegetable")
            .data,
        vec![1u8, 34, 35, 36]
    );

    let version_id = 1u8;
    let json_data = JsonDataVersion::from_u8(version_id).expect("valid version");
    let old_json_bytes = serde_json::to_vec(&TestStructV1::new(String::from("Test"))).unwrap();
    let json_bytes: Vec<u8> = json_data.migrate(old_json_bytes).expect("json migration failed");
    let migrated_json: TestStructV3 = serde_json::from_slice(&json_bytes).expect("deserialize failed");
    assert_eq!(migrated_json.field1, "Test");
    assert_eq!(migrated_json.field2, "Field2");
    assert_eq!(migrated_json.field3, "Field3");

    let version_id = 2u8;
    let json_data = JsonDataVersion::from_u8(version_id).expect("valid version");
    let old_json_bytes = serde_json::to_vec(&TestStructV2::new(String::from("Test1"), String::from("Test2"))).unwrap();
    let json_bytes: Vec<u8> = json_data.migrate(old_json_bytes).expect("json migration failed");
    let migrated_json: TestStructV3 = serde_json::from_slice(&json_bytes).expect("deserialize failed");
    assert_eq!(migrated_json.field1, "Test1");
    assert_eq!(migrated_json.field2, "Test2");
    assert_eq!(migrated_json.field3, "Field3");

    let version_id = 2u8;
    let json_data = JsonDataVersion::from_u8(version_id).expect("valid version");
    let old_json_bytes: Slice =
        serde_json::to_vec(&TestStructV2::new(String::from("Test1"), String::from("Test2"))).unwrap().into();
    let json_bytes: Vec<u8> = json_data.migrate(old_json_bytes).expect("json migration failed");
    let migrated_json: TestStructV3 = serde_json::from_slice(&json_bytes).expect("deserialize failed");
    assert_eq!(migrated_json.field1, "Test1");
    assert_eq!(migrated_json.field2, "Test2");
    assert_eq!(migrated_json.field3, "Field3");
}
