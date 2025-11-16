const { PrismaClient } = require('@prisma/client');

async function testDatabase() {
  console.log('🧪 Testing Database Integration...');
  
  const db = new PrismaClient();
  
  try {
    // Test connection
    await db.$connect();
    console.log('✅ Database connected successfully');
    
    // Test creating an astronaut
    const astronaut = await db.astronaut.create({
      data: {
        name: 'Test Astronaut',
        role: 'Commander',
        status: 'active'
      }
    });
    console.log('✅ Created astronaut:', astronaut.name);
    
    // Test creating vital signs
    const vitalSign = await db.vitalSign.create({
      data: {
        astronautId: astronaut.id,
        heartRate: 75,
        hrv: 50,
        bloodOxygen: 98,
        temperature: 36.8,
        respirationRate: 16,
        bloodPressureSys: 120,
        bloodPressureDia: 80,
        stressLevel: 25,
        hydration: 85
      }
    });
    console.log('✅ Created vital signs record');
    
    // Test creating a mission
    const mission = await db.mission.create({
      data: {
        scenarioId: 'test_mission',
        name: 'Test Mission',
        description: 'Testing database integration',
        location: 'Test Location',
        duration: '2 hours',
        difficulty: 'easy',
        status: 'active'
      }
    });
    console.log('✅ Created mission:', mission.name);
    
    // Test creating a rover
    const rover = await db.rover.create({
      data: {
        roverId: 'test-rover-1',
        name: 'Test Rover',
        battery: 85,
        signal: 92,
        status: 'active',
        task: 'Testing',
        locationX: 100,
        locationY: 200,
        efficiency: 90
      }
    });
    console.log('✅ Created rover:', rover.name);
    
    // Test creating network nodes
    const node1 = await db.bioNode.create({
      data: {
        nodeId: 'test_node_1',
        name: 'Test Node 1',
        type: 'human',
        coherence: 95,
        power: 300,
        phase: 0,
        connected: true
      }
    });
    
    const node2 = await db.bioNode.create({
      data: {
        nodeId: 'test_node_2',
        name: 'Test Node 2',
        type: 'aui',
        coherence: 90,
        power: 250,
        phase: 45,
        connected: true
      }
    });
    console.log('✅ Created bio nodes');
    
    // Test creating connection
    const connection = await db.connection.create({
      data: {
        sourceId: node1.id,
        targetId: node2.id,
        strength: 85,
        active: true,
        phase: 22
      }
    });
    console.log('✅ Created network connection');
    
    // Test creating alert
    const alert = await db.alert.create({
      data: {
        type: 'info',
        source: 'system',
        message: 'Database integration test successful'
      }
    });
    console.log('✅ Created alert');
    
    // Test querying data
    const astronauts = await db.astronaut.findMany({
      include: {
        vitalSigns: true,
        evaSessions: true,
        suitTelemetry: true
      }
    });
    console.log(`✅ Found ${astronauts.length} astronaut(s) with related data`);
    
    const networkStats = await db.bioNode.findMany({
      include: {
        connections: true,
        targetConnections: true
      }
    });
    console.log(`✅ Found ${networkStats.length} network nodes with connections`);
    
    console.log('\n🎉 Database integration test completed successfully!');
    console.log('All services are working correctly.');
    
  } catch (error) {
    console.error('❌ Database test failed:', error);
    process.exit(1);
  } finally {
    await db.$disconnect();
  }
}

testDatabase();