pub const PRELUDE_SOURCE: &str = r#"intern fn printInt(value: int): unit
intern fn print(value: string): unit
intern fn exit(code: int): unit
fn assert(value: bool): unit {
  if value { () } else { 
    print("ASSERT FAILED");
    exit(1)
  };
}
namespace char {
  intern fn to_string(self: char): string
}
type Array = {}
namespace Array {
  intern fn new<T>(len: int): Array<T>
  intern fn length(self: Array): int
}
namespace string {
  intern fn new(bytes: Array<char>): string
  intern fn length(self: string): int
  fn index_of(self: string, c: char): int {
    mut i = 0;
    mut ret = -1;
    while (ret == -1 and i < self.length()) {
      if (self[i] == c) {
        ret = i;
      };
      i = i + 1;
    };
    ret
  }
  fn concat(self: string, other: string): string {
    val new_length = self.length() + other.length();
    val copied = Array::new<char>(new_length);
    mut i = 0;
    while (i < self.length()) {
      copied[i] = self[i];
      i = i + 1;
    };
    i = 0;
    while (i < other.length()) {
      copied[i + self.length()] = other[i];
      i = i + 1;
    };
    new(copied)
  }
}
// -------- END PRELUDE --------"#;
pub const PRELUDE_LINES: usize = 47;
