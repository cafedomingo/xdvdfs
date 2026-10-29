import filecmp
import harness
import os
import tempfile

NAME_LEN = 18
FILE_BYTE_COUNT = 0xe + NAME_LEN
assert FILE_BYTE_COUNT == 32

CMD_EXISO_PACK_DIR = 'cargo r --bin xdvdfs -- extract-xiso -c {input} {output}'
CMD_EXISO_UNPACK_X = 'cargo r --bin xdvdfs -- extract-xiso -d {output} -x {input}'
CMD_EXISO_UNPACK_NONE = 'cargo r --bin xdvdfs -- extract-xiso -d {output} {input}'

# Cannot specify output image with extract-xiso, output must equal input
CMD_EXISO_REPACK = 'cargo r --bin xdvdfs -- extract-xiso -D -r {input}'

def rand_file_name():
    import string
    import random
    return ''.join(random.choice(string.ascii_lowercase) for _ in range(NAME_LEN))

class ExtractXisoCompat(harness.TestCase):
    def __init__(self):
        super().__init__(name='ExtractXisoCompat')

    def set_up(self, dir):
        with open(dir + '/' + rand_file_name(), 'w') as f:
            f.write('extract-xiso compat')

    def run(self, dir, args):
        img_file = tempfile.NamedTemporaryFile()
        harness.run_cmd_with_io(CMD_EXISO_PACK_DIR, dir, img_file.name)

        harness.test_pack_unpack(dir, img_file, CMD_EXISO_UNPACK_X)
        harness.test_pack_unpack(dir, img_file, CMD_EXISO_UNPACK_NONE)

        # extract-xiso repack must be tested in-place
        harness.run_cmd_with_io(CMD_EXISO_REPACK, img_file.name, img_file.name)
        harness.test_pack_unpack(dir, img_file, CMD_EXISO_UNPACK_X)
        

class EmptyFile(harness.PackTestCase):
    def __init__(self):
        super().__init__(name='EmptyFile')

    def set_up(self, dir):
        with open(dir + '/empty_file', 'w') as f:
            pass

class EmptyRoot(harness.PackTestCase):
    def __init__(self):
        super().__init__(name='EmptyRoot')

    def set_up(self, dir):
        pass

class EmptySubdir(harness.PackTestCase):
    def __init__(self):
        super().__init__(name='EmptySubdir')

    def set_up(self, dir):
        os.mkdir(dir + '/subdir')

class SpecialCharsInName(harness.PackTestCase):
    def __init__(self):
        super().__init__(name='SpecialCharsInName')

    def create_file(self, dir, name):
        with open(dir + '/' + name, 'w') as f:
            f.write(name)

    def set_up(self, dir):
        self.create_file(dir, 'Ü')
        self.create_file(dir, 'b')
        self.create_file(dir, 'ü')
        self.create_file(dir, 'á')

class ManyFiles(harness.PackTestCase):
    def __init__(self):
        super().__init__(name='ManyFiles')

    def set_up(self, dir):
        os.mkdir(dir + '/a')
        max_offset = 65536
        num_files = max_offset // FILE_BYTE_COUNT + 1
        for i in range(num_files):
            name = rand_file_name()
            with open(dir + '/a/' + name, 'w') as f:
                f.write('data')

class DirentSize2048(harness.PackTestCase):
    def __init__(self):
        super().__init__(name='DirentSize2048')

    def set_up(self, dir):
        os.mkdir(dir + '/a')
        with open(dir + '/b', 'w') as f:
            f.write('data')

        for i in range(2048 // FILE_BYTE_COUNT):
            name = rand_file_name()
            with open(dir + '/a/' + name, 'w') as f:
                f.write('data')

class CopyOut(harness.TestCase):
    def __init__(self):
        super().__init__(name='CopyOut')

    def set_up(self, dir):
        os.mkdir(dir + '/a')

        with open(dir + '/b', 'w') as f:
            f.write('root_b')

        with open(dir + '/a/f1', 'w') as f:
            f.write('root_a_f1')
        with open(dir + '/a/f2', 'w') as f:
            f.write('root_a_f2')

    def run(self, dir, args):
        img_file = tempfile.NamedTemporaryFile()
        harness.run_cmd_with_io(args.pack_cmd, dir, img_file.name)

        # options is the image data path
        output_dir = tempfile.TemporaryDirectory()
        copyout_cmd = 'cargo r --bin xdvdfs -- copy-out {input} {options} {output}'
        harness.run_cmd_with_io(copyout_cmd, img_file.name, output_dir.name + '/b', '/b')
        harness.run_cmd_with_io(copyout_cmd, img_file.name, output_dir.name + '/a', '/a')

        l1_cmp = filecmp.dircmp(dir, output_dir.name)
        if l1_cmp.diff_files or l1_cmp.left_only or l1_cmp.right_only or l1_cmp.funny_files:
            l1_cmp.report_full_closure()
            raise Exception('Mismatch between test case input and packed output')
