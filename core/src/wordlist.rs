//! A 256-word list shared by the short-code pairing (one word per byte of PAKE
//! secret) and identity fingerprints (one word per byte of a hash). Having
//! exactly 256 entries means every byte maps to a word with no bias.

/// 256 short, distinct words. Index a word by any `u8`.
#[rustfmt::skip]
pub(crate) const WORDS: [&str; 256] = [
    "acid","acorn","album","amber","anvil","apple","apron","arch","arena","armor",
    "ash","aspen","atlas","attic","axle","bacon","badge","bagel","bamboo","banjo",
    "barn","basil","bay","beacon","beam","bean","bear","beetle","bell","berry",
    "birch","bison","blade","blaze","bloom","board","boat","bolt","bongo","bonus",
    "boot","boulder","brave","bread","brick","bridge","broom","brush","bubble","bucket",
    "buffalo","bugle","bulb","bundle","cabin","cable","cactus","camel","candle","canoe",
    "canvas","canyon","cape","cargo","carol","carrot","castle","cave","cedar","cell",
    "chalk","cherry","chess","chime","cider","cinder","cliff","cloak","clover","cluster",
    "coal","cobra","cocoa","comet","copper","coral","cotton","cove","crane","crater",
    "crayon","creek","crest","crow","crown","cube","dagger","daisy","dawn","delta",
    "denim","desk","diamond","dingo","dock","dolphin","donut","dove","dragon","drum",
    "dune","eagle","ember","emu","engine","fable","falcon","fang","fern","ferry",
    "fiber","field","fig","finch","flame","flask","flint","flute","forest","fox",
    "frost","garlic","gecko","ginger","glacier","globe","glove","gnome","goat","gold",
    "grape","grotto","guitar","hammer","harbor","hawk","hazel","hedge","helm","heron",
    "hive","honey","horn","hut","igloo","indigo","ivory","ivy","jaguar","jasmine",
    "jelly","jet","jewel","kayak","kelp","kettle","key","kiwi","koala","lagoon",
    "lantern","lark","laurel","leaf","ledger","lemon","lentil","lily","lime","linen",
    "lion","llama","lobster","locket","lotus","lynx","mango","maple","marble","marsh",
    "meadow","melon","mesa","meteor","mint","mist","moss","moth","mule","nectar",
    "needle","nest","nettle","nickel","noble","nomad","oak","oasis","ocean","olive",
    "onyx","opal","orbit","otter","owl","oxide","paddle","palm","panda","papaya",
    "parrot","peach","pearl","pebble","pepper","phoenix","pigeon","pillow","pine","piston",
    "plum","pond","poppy","prairie","puma","quartz","quill","quilt","radish","raft",
    "rapid","raven","reef","ribbon","ridge","river","robin","rocket","rose","rubble",
    "ruby","sable","saffron","sage","salmon","sand",
];
