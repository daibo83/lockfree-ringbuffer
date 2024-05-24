pub struct BoundedBuffer<T> {
    size: usize,
    data: Vec<T>,
    reading: bool,
}

pub struct ReadToken;

impl<T> BoundedBuffer<T> {
    pub fn new(size: usize) -> Self {
        assert!(size > 0);
        BoundedBuffer { size, data: vec![], reading: false }
    }

    pub fn write(&mut self, x: T) -> Result<(), ()> {
        if self.data.len() < self.size {
            self.data.push(x);
            Ok(())
        } else {
            if self.reading {
                Err(())
            } else {
                self.data.rotate_left(1);
                self.data[self.size - 1] = x;
                Ok(())
            }
        }
    }

    pub fn read(&mut self) -> Result<(), ()> {
        if self.reading {
            Err(())
        } else {
            self.reading = true;
            Ok(())
        }
    }

    pub fn stop_reading(&mut self) -> Result<(), ()> {
        if !self.reading {
            Err(())
        } else {
            self.reading = false;
            Ok(())
        }
    }

    pub fn next(&mut self) -> Result<Option<T>, ()> {
        if !self.reading {
            Err(())
        } else {
            if self.data.is_empty() {
                Ok(None)
            } else {
                self.data.rotate_left(1);
                Ok(self.data.pop())
            }
        }
    }

}
